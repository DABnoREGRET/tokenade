//! Dedicated per-site readers for pages the NATIVE fetcher measurably fails
//! on. Same selective philosophy as the WebFetch gate: intercept only where
//! the native path returns a bot-wall or an empty JS shell, so the reader is
//! strictly better than what the agent would otherwise get.
//!
//! Current plans:
//! * **Reddit** — threads and listings go through the **Atom feed**
//!   (`…/.rss`), and it is the only anonymous source left. MEASURED
//!   2026-09-11: `old.reddit.com` answers **302 to `/login/?reason=lor2`**
//!   for a thread as for a listing, `www.reddit.com` serves instead an 8 KB
//!   "Please wait for verification" interstitial from which the delivered
//!   page extracts **0 characters**, and the public `.json` endpoints answer
//!   403. The `.rss` answers 200: 49 KB and 25 entries for `/r/rust/`, with
//!   author, date, link and full body. A THREAD's feed carries the submission
//!   **then its comments**, one entry each, titled `/u/<author> on <thread>`.
//!   Both readers keep their `old.reddit` path: the SHAPE of the body
//!   decides, so a mirror or a session that still serves that HTML is still
//!   read.
//! * **crates.io** — a pure SPA (32 extractable chars); the public
//!   registry API carries the description, versions and links.
//! * **TikTok** — zero extractable characters from 376 KB, because every
//!   visible string is written client-side from the page's own
//!   `__UNIVERSAL_DATA_FOR_REHYDRATION__` payload, which the reader parses.
//! * **GitHub issue/PR lists** — the React list distilled to 765 characters of
//!   furniture with no issue title in it; the public search API speaks the
//!   same query language the URL carries, so the site's filter is forwarded.
//!
//! Sites the dashboard asks for that NO http-only reader can serve, so nobody
//! re-tries them: facebook.com and slack.com/app.slack.com are behind a login;
//! amazon.com product pages answer a "continue shopping" bot wall (324 chars,
//! MEASURED on `/dp/…`) that only a cookied browser session clears; and
//! quora.com ships its answers as empty stubs to logged-out clients (see the
//! note on [`BRITANNICA`]'s neighbours below).
//!
//! Fail-open contract: [`plan`] only proposes; if the alternate fetch or
//! [`render`] fails (layout drift, API change, thin result) the caller MUST
//! fall back to the generic pipeline on the original URL — a reader can
//! upgrade quality, never gate access.

mod article;
mod misc_sites;
mod product;
pub use article::*;
use misc_sites::*;
pub use product::*;

use regex::Regex;

mod dev_reference;
mod fediverse;
mod feed;
mod forges;
mod notion;
mod reddit;
mod youtube;
pub(crate) use dev_reference::*;
pub(crate) use fediverse::*;
pub use feed::*;
pub use forges::*;
pub use notion::*;
use reddit::*;
pub use youtube::*;

/// How to fetch + interpret a recognised URL.
pub struct Plan {
    /// Alternate representation to fetch instead of the original URL.
    pub fetch_url: String,
    /// `Some(json)` turns the primary fetch into a `POST` of that body with
    /// `Content-Type: application/json` (Notion's `loadPageChunk` has no GET
    /// form). `None` = plain GET. Never applies to `aux_url`.
    pub post_body: Option<String>,
    /// Optional SECOND fetch (comments / thread context) whose body the
    /// caller appends after [`AUX_SEP`] so the pure renderer can split them.
    /// `None` = single-fetch reader.
    pub aux_url: Option<String>,
    pub kind: Kind,
    /// Short label for banners/telemetry (e.g. `old.reddit.com`).
    pub via: &'static str,
}

impl Plan {
    /// Plain GET of an alternate representation — every reader but Notion.
    fn get(fetch_url: String, kind: Kind, via: &'static str) -> Plan {
        Plan {
            fetch_url,
            post_body: None,
            aux_url: None,
            kind,
            via,
        }
    }

    /// POST the given JSON body instead of a GET (Notion's `loadPageChunk`
    /// has no GET form). Never applies to `aux_url` — that stays a GET.
    fn post(fetch_url: String, body: String, kind: Kind, via: &'static str) -> Plan {
        Plan {
            post_body: Some(body),
            ..Plan::get(fetch_url, kind, via)
        }
    }

    /// Add the SECOND fetch (comments / thread context) whose body the caller
    /// appends after [`AUX_SEP`].
    fn with_aux(self, aux_url: String) -> Plan {
        Plan {
            aux_url: Some(aux_url),
            ..self
        }
    }
}

/// Separator the hook inserts between the primary body and the aux body when
/// a plan has an `aux_url`, so a pure renderer can `split_once` them.
pub const AUX_SEP: &str = "\u{1e}tokenade-aux\u{1e}";

pub enum Kind {
    GithubRepo,
    RedditThread,
    RedditListing,
    TikTok,
    GithubIssueSearch,
    CratesApi,
    HnItem,
    DiscourseTopic,
    StackExchange,
    YouTube,
    XPost,
    Mastodon,
    Bluesky,
    Lemmy,
    NotionPage,
    GithubIssue,
    GithubBlob(&'static str),
    GithubActionsRun,
    GitlabThread,
    FeedDocument,
    HuggingFaceRepo,
    IndeedJobs,
    MasterclassArticle,
    /// An article page on a site with a measured chrome (or metadata) problem.
    /// The spec is `'static` because every site recipe is a compile-time
    /// constant — see [`ArticleSpec`].
    Article(&'static ArticleSpec),
}

/// Reader outputs below this size are treated as a failed parse (layout
/// drift) and the caller falls back to the generic pipeline.
const READER_VALID_MIN_CHARS: usize = 200;

use crate::regexes::re;

/// `(host, path+query)` of an http(s) URL, host lowercased, port/userinfo
/// stripped. Same minimal parsing stance as `net::scheme_host`.
fn host_path(url: &str) -> Option<(String, String)> {
    let rest = url.split_once("://")?.1;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some((host, path.to_string()))
}

/// Recognise a URL a dedicated reader handles better than the native
/// fetcher. `None` = no opinion, use the generic pipeline.
pub fn plan(url: &str) -> Option<Plan> {
    let (host, path) = host_path(url)?;
    let h = host.strip_prefix("www.").unwrap_or(&host);
    match h {
        // Notion serves every page — public or not — as the same ~18 KB JS
        // shell whose only metadata is Notion's generic marketing copy, so
        // the native fetcher extracts NOTHING page-specific (MEASURED). The
        // unauthenticated `loadPageChunk` API returns the real blocks for
        // any publicly-shared page; a private one answers with an error body
        // the renderer rejects, falling back to the generic pipeline.
        "notion.so" | "app.notion.com" | "notion.site" => notion_plan(&path),
        _ if h.ends_with(".notion.site") => notion_plan(&path),
        // A THREAD: `/r/<sub>/comments/<id>/…` or the bare `/comments/<id>`.
        // Anchored rather than a `contains("/comments/")` test, which also
        // swallowed `/user/<name>/comments/` — a LISTING of someone's comments,
        // which the arm below renders properly.
        "reddit.com" | "old.reddit.com" | "np.reddit.com" | "new.reddit.com"
        | "sh.reddit.com" | "i.reddit.com" | "amp.reddit.com"
            if re(r"^(/r/[^/]+)?/comments/[A-Za-z0-9]{4,}").is_match(&path) => reddit::reddit_thread_plan(&path),
        // A LISTING: subreddit front pages, sorted views, user pages and
        // search. The modern frontend answers these with an 8 KB
        // "Please wait for verification" interstitial — MEASURED on
        // /r/rust/, /r/rust/top/?t=week and /r/programming/, all three
        // distilling to ZERO characters — while old.reddit serves the same
        // listing as 140 KB of server-rendered HTML. Same swap the thread
        // reader already makes, for the URL shapes it never covered.
        //
        // The query is KEPT here, unlike the thread arm: on a listing it is
        // the request itself (`?t=week`, `?q=…&restrict_sr=1`, `?after=…`),
        // not tracking noise.
        "reddit.com" | "old.reddit.com" | "np.reddit.com" | "new.reddit.com"
        | "sh.reddit.com" | "i.reddit.com" | "amp.reddit.com" if is_reddit_listing(&path) => reddit::reddit_listing_plan(&path),
        // TikTok serves a pure JS shell: MEASURED, the native fetcher
        // recovered ZERO characters from 281 KB (profile) and 376 KB (video)
        // of HTML. Every page nonetheless embeds
        // `__UNIVERSAL_DATA_FOR_REHYDRATION__`, which already holds the
        // caption, author, hashtags and counts as structured data.
        "tiktok.com" | "m.tiktok.com" | "vt.tiktok.com" | "vm.tiktok.com"
            if re(r"^/@[^/]+(/(video|photo)/\d+)?/?$").is_match(path.split(['?', '#']).next().unwrap_or(&path)) => misc_sites::tiktok_plan(&path),
        // YouTube: the watch page is a JS shell to the native fetcher, but it
        // embeds `ytInitialPlayerResponse` with the title, channel, stats and
        // full description — the reader mines that. (Transcripts now sit
        // behind a proof-of-origin token wall, so they're out of scope for an
        // HTTP-only reader; the description usually carries the summary/
        // chapters an agent asked for.)
        "youtube.com" | "m.youtube.com" | "music.youtube.com" => youtube::youtube_watch_plan(&path),
        "youtu.be" => youtube::youtu_be_plan(&path),
        // X / Twitter: the public syndication endpoint (no auth, stable for
        // years — used by every embed) returns the post JSON. Replies are not
        // exposed by this endpoint, so the reader serves the post + author +
        // engagement; the native fetcher gets only a JS shell.
        "twitter.com" | "x.com" | "mobile.twitter.com" | "nitter.net" => misc_sites::twitter_plan(&path),
        // Bluesky: the AT-Protocol AppView is fully public (no auth). One
        // call to getPostThread returns the post AND its reply tree; the
        // handle resolves inside the at-uri, so no separate DID lookup.
        "bsky.app" => misc_sites::bluesky_plan(&path),
        // Lemmy (fediverse Reddit): public API, post id in the URL path.
        // Post + top comments in two calls.
        "lemmy.world" | "lemmy.ml" | "sh.itjust.works" | "beehaw.org" | "programming.dev"
        | "lemm.ee" | "sopuli.xyz" | "feddit.de"
            if re(r"^/post/(\d+)").is_match(&path) => misc_sites::lemmy_plan(&host, &path),
        // Mastodon (fediverse): federated across arbitrary hosts, so it's
        // recognised by the URL SHAPE `/@user/<snowflake-id>` rather than a
        // host allow-list. A false positive (a non-Mastodon site using the
        // same shape) fails the API fetch/parse and falls open to generic.
        // Post via /statuses/<id>, replies via /statuses/<id>/context.
        _ if re(r"^/@[^/]+/(\d{6,})$").is_match(&path) => misc_sites::mastodon_status_plan(&host, &path),
        "crates.io" => product::crates_io_plan(&path),
        // HN item pages rate-limit plain fetchers aggressively (429 measured
        // 2026-07-10) and truncate long threads behind "more comments"; the
        // official Algolia API serves the whole tree in one call.
        // GitHub issues + PRs: the HTML server-renders the opening post but
        // LAZY-LOADS the thread — measured, the distilled page was 2.4 KB
        // whose tail was repeated "Uh oh! There was an error while loading."
        // placeholders instead of the 255 comments. The REST API returns the
        // real thread unauthenticated (60 req/h per IP; over that it answers
        // 403 and the renderer's `None` falls back to the generic pipeline).
        // A `blob` page is a JavaScript code viewer: the file itself is one
        // fetch away on raw.githubusercontent.com, which is what an agent
        // asking about a source file actually wants. The downstream inline
        // cap bounds a large file, so a reader here does not blow the budget.
        "github.com" if re(r"^/[^/]+/[^/]+/blob/").is_match(&path) => forges::github_blob_plan(&path),
        // An Actions run page distils to ~344 tokens of nothing (MEASURED on
        // a real run: 52 466 tokens of HTML in, no job or step detail out).
        // The REST API carries the jobs, and — the point of visiting at all —
        // which STEP failed.
        "github.com" if re(r"^/[^/]+/[^/]+/actions/runs/\d+").is_match(&path) => forges::github_actions_run_plan(&path),
        // GitLab issues/MRs: the page distils to 243 tokens of shell
        // (MEASURED). The v4 API serves the thread body unauthenticated —
        // but NOT its comments (`/notes` answers 401), so this reader
        // deliberately delivers the description alone.
        "gitlab.com" if re(r"^/.+/-/(issues|merge_requests)/\d+").is_match(&path) => forges::gitlab_issue_plan(&path),
        "github.com" if re(r"^/[^/]+/[^/]+/(issues|pull)/\d+").is_match(&path) => forges::github_issue_plan(&path),
        // Issue / PR LISTS and their filtered searches. The detail arms above
        // claim `/issues/<n>` and `/pull/<n>` first, so what reaches here is
        // the React-rendered list, which the native fetcher cannot see:
        // MEASURED, `/rust-lang/rust/issues?q=is:issue+label:A-lint` distilled
        // to 765 characters of page furniture with not one issue title in it.
        // The public search API answers the SAME query language the URL
        // carries, so the site's own filter string is forwarded verbatim.
        "github.com" if re(r"^/[^/]+/[^/]+/(issues|pulls)/?([?#]|$)").is_match(&path) => forges::github_issue_list_plan(&path),
        // A repository's HOME page, the most requested GitHub address and the
        // only one that had no reader. MEASURED 2026-09-12 on
        // `github.com/rust-lang/regex`: title, description, two "Uh oh!
        // There was an error while loading", the Fork/Star buttons, a commit
        // count — and not one line of README, which is rendered client-side.
        // The arms above claim `/blob/`, `/actions/runs/`, `/issues/<n>` and
        // the lists first, so what arrives here really is `/{owner}/{repo}`.
        "github.com" if re(r"^/[^/]+/[^/]+/?([?#]|$)").is_match(&path) => forges::github_repo_plan(&path),
        // Google Docs has NO reader on purpose: `export?format=txt` 404s on a
        // published doc (that endpoint exists only for the /document/d/{fileId}
        // form) and the /pub page is served behind a cookie-consent wall that
        // distils to 141 tokens of "authorise cookies". MEASURED on two real
        // published documents. Do not add one back without new evidence.
        // Hugging Face models + datasets. The path is deliberately narrow:
        // `/{owner}/{name}` also matches docs, blog and papers URLs, which the
        // generic pipeline already renders fine.
        "huggingface.co" if re(r"^/datasets/[^/]+/[^/]+/?$").is_match(&path) => product::hf_dataset_plan(&path),
        // Canonical models are `/{owner}/{name}`, but the classics are
        // published at a bare `/{name}` (`/bert-base-uncased`) — a single
        // segment that the two-segment pattern below silently missed.
        "huggingface.co"
            if re(r"^/[^/]+/?$").is_match(&path)
                && !re(r"^/(docs|blog|papers|spaces|datasets|models|collections|posts|join|login|settings|pricing|tasks|chat|new)/?$")
                    .is_match(&path) => product::hf_org_plan(&path),
        "huggingface.co"
            if re(r"^/[^/]+/[^/]+/?$").is_match(&path)
                && !re(r"^/(docs|blog|papers|spaces|datasets|models|collections|posts|join|login|settings|pricing)/")
                    .is_match(&path) => product::hf_repo_plan(&path),
        // RSS / Atom feeds, recognised by URL shape. A feed is a FORMAT, not a
        // host: matching the conventional suffixes covers the overwhelming
        // majority without a content sniff, and `feed_document` returns None
        // on anything that is not really a feed, so a false match costs one
        // fallback to the generic pipeline.
        // Match on the path WITHOUT its query: `/feed/?utm_source=rss` and
        // `/index.rss?x=1` are common real shapes that an anchored `$` missed.
        _ if re(r"(?i)(\.(rss|atom)$|\.xml$|/(feed|rss|atom)/?$)")
            .is_match(path.split(['?', '#']).next().unwrap_or(&path))
            || re(r"(?i)[?&]feed=").is_match(&path) => feed::feed_plan(url),
        "news.ycombinator.com" if path.starts_with("/item") => misc_sites::hn_item_plan(&path),
        // Stack Exchange Q&A: the HTML is server-rendered and rich, but a
        // GENERIC extraction loses the Q&A structure (answers carry no
        // headings), so prompt-scoped retrieval keeps landing on the
        // question instead of the answers (measured). The reader re-fetches
        // the same page and rebuilds it as `## Question` / `## Answer
        // (N votes[, accepted])` sections, votes-descending.
        "stackoverflow.com" | "superuser.com" | "serverfault.com" | "askubuntu.com"
            if re(r"^/(questions|q)/\d+").is_match(&path) => misc_sites::stack_exchange_plan(&host, &path),
        // Article pages. These readers re-fetch the SAME url — the win is not
        // an alternate representation but keeping only the article. Each arm
        // matches the site's ARTICLE path shape only: a homepage or a section
        // index has no article to extract and must fall through to the generic
        // pipeline, not render as an empty one.
        "britannica.com"
            if re(r"^/(biography|topic|place|science|animal|technology|art|plant|sports|money)/[A-Za-z0-9][A-Za-z0-9\-]*(/[A-Za-z0-9\-]+)?/?([?#].*)?$")
                .is_match(&path) => article::britannica_plan(&host, &path),
        // Indeed job SEARCH pages, every locale host (fr./uk./de./www.). Two
        // shapes in the wild: the query form `/jobs?q=…&l=…` and the localised
        // slug form `/q-<query>-l-<place>-emplois.html` (`-jobs.html` in
        // English). NOT `/career-advice/…`, which is an article the generic
        // pipeline already renders to 91% body — measured, so it stays out.
        _ if h.ends_with("indeed.com")
            && (re(r"^/jobs([?#]|$)").is_match(&path)
                || re(r"^/(q|l)-.*\.html([?#].*)?$").is_match(&path)) => misc_sites::indeed_plan(&host, &path),
        "masterclass.com" if re(r"^/articles/[a-z0-9\-]+/?([?#].*)?$").is_match(&path) => article::masterclass_plan(&host, &path),
        "cbsnews.com" if re(r"^/news/[a-z0-9\-]+/?([?#].*)?$").is_match(&path) => article::cbsnews_plan(&host, &path),
        "ign.com" if re(r"^/articles/[a-z0-9\-]+/?([?#].*)?$").is_match(&path) => article::ign_plan(&host, &path),
        // `/<numeric id>/<Title_With_Underscores>` is academia.edu's paper URL;
        // profile and topic pages have other shapes and hold no abstract.
        "academia.edu" if re(r"^/\d{6,}/[^/?#]+([?#].*)?$").is_match(&path) => article::academia_plan(&host, &path),
        _ if h.ends_with(".stackexchange.com") && re(r"^/(questions|q)/\d+").is_match(&path) => misc_sites::stack_exchange_plan(&host, &path),
        // Discourse topic URLs share a distinctive `/t/<slug>/<id>` shape
        // across every Discourse forum (users.rust-lang.org, discuss.python
        // .org, …). The HTML is a JS shell (content only in <noscript>); the
        // canonical `.json` view carries every post. Host-agnostic on
        // purpose — a non-Discourse hit parses to None and falls back.
        _ if re(r"^/t/[^/]+/\d+").is_match(&path) => misc_sites::discourse_plan(&host, &path),
        _ => None,
    }
}

/// Render the fetched alternate representation to markdown. `None` (parse
/// failure or thin output) MUST route the caller back to the generic
/// pipeline on the original URL.
/// Does this reader fetch a real HTML PAGE of the same content the agent
/// asked for, rather than an alternate machine representation?
///
/// The distinction drives the SAVINGS CREDIT, not the rendering. Swapping
/// new-reddit for old-reddit really does compress an HTML page the agent
/// would otherwise have carried, so `raw_html → markdown` is an honest
/// assiette. A reader that calls a REST API or `raw.githubusercontent.com`
/// compresses nothing the agent would ever have seen — crediting that payload
/// would invent a saving. See `coefficients::reader_credit_savings`.
pub fn fetches_html_page(kind: &Kind) -> bool {
    matches!(
        kind,
        // An article reader fetches the SAME html page the agent would have
        // carried and hands back only the article, so the compression is real
        // and the web model applies.
        Kind::RedditThread
            | Kind::RedditListing
            | Kind::TikTok
            | Kind::StackExchange
            | Kind::YouTube
            | Kind::Article(_)
            | Kind::IndeedJobs
            | Kind::MasterclassArticle
    )
}

pub fn render(kind: &Kind, body: &str) -> Option<String> {
    let md = match kind {
        Kind::GithubRepo => forges::github_repo(body),
        Kind::RedditThread => reddit_thread(body),
        Kind::RedditListing => reddit_listing(body),
        Kind::TikTok => tiktok_page(body),
        Kind::GithubIssueSearch => github_issue_search(body),
        Kind::CratesApi => crates_api(body),
        Kind::HnItem => hn_item(body),
        Kind::DiscourseTopic => discourse_topic(body),
        Kind::StackExchange => stackexchange_page(body),
        Kind::YouTube => youtube_watch(body),
        Kind::XPost => x_post(body),
        Kind::Mastodon => mastodon_status(body),
        Kind::Bluesky => bluesky_thread(body),
        Kind::Lemmy => lemmy_post(body),
        Kind::NotionPage => notion_page(body),
        Kind::GithubIssue => github_issue(body),
        Kind::GithubBlob(lang) => github_blob(body, lang),
        Kind::GithubActionsRun => github_actions_run(body),
        Kind::GitlabThread => gitlab_thread(body),
        Kind::FeedDocument => feed_document(body),
        Kind::HuggingFaceRepo => huggingface_repo(body),
        Kind::Article(spec) => article_page(spec, body),
        Kind::IndeedJobs => indeed_jobs(body),
        Kind::MasterclassArticle => masterclass_article(body),
    }?;
    // A short render is a failed parse for the HTML-derived readers, but a
    // legitimately complete post for structured JSON ones — a tweet can be
    // 20 chars. Gate the thin-parse floor per kind.
    let floor = match kind {
        Kind::XPost => 40,
        // A GREEN Actions run is legitimately terse — name, conclusion, one
        // job line — and lands under the generic floor even though the parse
        // was perfect (MEASURED: a passing run rendered ~150 chars and was
        // rejected). Structured-JSON readers cannot use a length floor as a
        // parse check the way the HTML-derived ones do.
        Kind::GithubActionsRun | Kind::GitlabThread | Kind::HuggingFaceRepo => 60,
        // A repository card WITHOUT a README is three lines — name,
        // description, counters — and that is a perfect parse of what the API
        // carries.
        Kind::GithubRepo => 60,
        // Same exception, from the other direction: a TikTok profile is
        // legitimately three lines (handle, one-line bio, counts) and a search
        // that matched nothing is one. Both come out of a payload the reader
        // parsed correctly, so length cannot stand in for a parse check.
        Kind::TikTok | Kind::GithubIssueSearch => 60,
        // Same class of exception, other site: a thread the submission itself
        // declares as having no comments is legitimately short — title, meta,
        // OP selftext — and the generic floor rejected it as a failed parse,
        // which is what a "reader broke" row on a fresh thread really was. A
        // walled or JS-shell reddit page carries no `data-comments-count` at
        // all, so it still fails open.
        Kind::RedditThread if body.contains(r#"data-comments-count="0""#) => 60,
        // And since both reddit plans request the Atom feed: a PARSED feed is
        // a successful parse, whatever its size. A thread whose submission is
        // a plain link is three lines — title, author, address — and the
        // generic floor rejected it as a broken reader, which sent the page
        // back to the 8 KB interstitial from which nothing is extracted. A
        // wall or a JS shell is not a feed, so this path still fails open.
        Kind::RedditThread | Kind::RedditListing if reddit::is_a_feed(body) => 60,
        _ => READER_VALID_MIN_CHARS,
    };
    (md.chars().count() >= floor).then_some(md)
}

#[cfg(test)]
include!("site_readers_tests.rs");
