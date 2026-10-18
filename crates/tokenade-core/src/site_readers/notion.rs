//! Notion pages, read through the unauthenticated `loadPageChunk` API.
//!
//! Every Notion URL — public or not — serves the same ~18 KB JS shell, so the
//! page itself carries nothing. The API returns the real blocks for anything
//! publicly shared; a private page answers with an error body this renderer
//! rejects, and the caller falls back to the generic pipeline.

use super::*;

/// Plan for a Notion page path, shared by the `notion.site` custom-domain
/// arm and the canonical hosts.
pub(super) fn notion_plan(path: &str) -> Option<Plan> {
    let id = notion_page_id(path)?;
    Some(Plan::post(
        format!("{NOTION_API}loadPageChunk"),
        notion_chunk_body(&id, 0, "{\"stack\":[]}"),
        Kind::NotionPage,
        "notion.so/api/v3",
    ))
}

/// Notion page id from a URL path: `/p/Some-Slug-<32hex>`, `/<32hex>`, or
/// `/Some-Slug-<32hex>`, re-hyphenated into the 8-4-4-4-12 form the API
/// demands. Anchored on the TAIL — a slug's words contribute stray hex
/// letters (`cafe`, `dead`), but the id is always the final 32 hex digits.
pub(super) fn notion_page_id(path: &str) -> Option<String> {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    let last = clean.trim_end_matches('/').rsplit('/').next()?;
    let hex: String = last.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() < 32 {
        return None;
    }
    let id = &hex[hex.len() - 32..];
    Some(format!(
        "{}-{}-{}-{}-{}",
        &id[0..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..32]
    ))
}

/// Payload part that pins which block is the page the agent asked for. The
/// fetcher prepends it because only the fetcher knows the requested URL, and
/// the follow-up fetches drag in unrelated parent pages.
pub fn notion_root_part(page_id: &str) -> String {
    format!("{{\"tokenadePageId\":\"{page_id}\"}}")
}

/// [`notion_page_id`] for a full URL — the follow-up chunk fetches need the
/// id again after the plan has been built.
pub fn notion_page_id_for_url(url: &str) -> Option<String> {
    let (_, path) = host_path(url)?;
    notion_page_id(&path)
}

/// `loadPageChunk` body for chunk `n`. `cursor` is the raw `cursor` object
/// the previous chunk returned (`{"stack":[]}` to start) — Notion streams a
/// long page in ~100-block chunks and collapsed toggles only arrive in the
/// later ones, so the caller loops until the stack empties.
pub fn notion_chunk_body(id: &str, chunk: usize, cursor: &str) -> String {
    format!(
        "{{\"pageId\":\"{id}\",\"limit\":100,\"cursor\":{cursor},\
         \"chunkNumber\":{chunk},\"verticalColumns\":false}}"
    )
}

/// The `cursor` object to feed the next `loadPageChunk`, or `None` when the
/// page is fully drained (empty `stack`).
pub fn notion_next_cursor(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let cursor = v.get("cursor")?;
    let stack = cursor.get("stack")?.as_array()?;
    (!stack.is_empty()).then(|| cursor.to_string())
}

/// `queryCollection` bodies for every database embedded in the chunks
/// fetched so far. Database rows are NOT part of `loadPageChunk` — without
/// this second call a roadmap/wiki page (the dominant public Notion shape)
/// renders as a title and nothing else.
pub fn notion_collection_queries(chunks: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = Default::default();
    for part in chunks.split(AUX_SEP) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(part) else {
            continue;
        };
        let Some(blocks) = v
            .get("recordMap")
            .and_then(|r| r.get("block"))
            .and_then(|b| b.as_object())
        else {
            continue;
        };
        for raw in blocks.values() {
            let Some(b) = raw.get("value").and_then(|v| v.get("value")) else {
                continue;
            };
            let ty = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if ty != "collection_view" && ty != "collection_view_page" {
                continue;
            }
            let (Some(cid), Some(vid)) = (
                b.get("collection_id").and_then(|c| c.as_str()),
                b.get("view_ids")
                    .and_then(|v| v.as_array())
                    .and_then(|v| v.first())
                    .and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            // `spaceId` rides on the block's collection pointer; the API
            // rejects the query without it.
            let space = b
                .get("format")
                .and_then(|f| f.get("collection_pointer"))
                .and_then(|p| p.get("spaceId"))
                .and_then(|s| s.as_str())
                .unwrap_or_default();
            if !seen.insert(cid.to_string()) {
                continue;
            }
            out.push(format!(
                "{{\"source\":{{\"type\":\"collection\",\"id\":\"{cid}\",\"spaceId\":\"{space}\"}},\
                 \"collectionView\":{{\"id\":\"{vid}\",\"spaceId\":\"{space}\"}},\
                 \"loader\":{{\"reducers\":{{\"collection_group_results\":\
                 {{\"type\":\"results\",\"limit\":{NOTION_ROWS_CAP}}}}},\"searchQuery\":\"\",\
                 \"userTimeZone\":\"UTC\",\"type\":\"reducer\"}}}}"
            ));
        }
    }
    out
}

/// Flatten a Notion rich-text property (`[[text, [[format, arg]…]]…]`) to
/// plain text, keeping inline link targets.
pub(super) fn notion_text(prop: Option<&serde_json::Value>) -> String {
    let Some(arr) = prop.and_then(|p| p.as_array()) else {
        return String::new();
    };
    let mut out = String::new();
    for seg in arr {
        let Some(seg) = seg.as_array() else { continue };
        // Non-string leads are inline mentions (user/date/page refs) with no
        // textual payload worth surfacing.
        let Some(t) = seg.first().and_then(|v| v.as_str()) else {
            continue;
        };
        out.push_str(t);
        let mut link = None;
        if let Some(fmts) = seg.get(1).and_then(|v| v.as_array()) {
            for f in fmts {
                let Some(f) = f.as_array() else { continue };
                if f.first().and_then(|v| v.as_str()) == Some("a") {
                    link = f.get(1).and_then(|v| v.as_str());
                }
            }
        }
        if let Some(u) = link {
            out.push_str(&format!(" <{u}>"));
        }
    }
    out
}

/// Inner block values keyed by id, merged across every fetched payload.
pub(super) type NotionBlocks<'a> = std::collections::HashMap<&'a str, &'a serde_json::Value>;

/// Collection (database) definitions keyed by id — `name` + `schema`.
pub(super) type NotionCollections<'a> = std::collections::HashMap<&'a str, &'a serde_json::Value>;

pub(super) struct NotionDoc<'a> {
    blocks: NotionBlocks<'a>,
    collections: NotionCollections<'a>,
}

/// Render one embedded database as a markdown table. Rows arrive from
/// `queryCollection` as `page` blocks parented to the collection, with
/// properties keyed by opaque schema ids that only `collection.schema`
/// resolves to human column names.
pub(super) fn notion_collection_table(doc: &NotionDoc<'_>, collection_id: &str) -> String {
    let Some(coll) = doc.collections.get(collection_id) else {
        return String::new();
    };
    let name = notion_text(coll.get("name"));
    let Some(schema) = coll.get("schema").and_then(|s| s.as_object()) else {
        return String::new();
    };
    // `title` is the row's own name and always leads; the rest keep the
    // schema's declaration order, which mirrors the table's columns.
    let mut cols: Vec<(&str, String)> = Vec::new();
    for (key, def) in schema {
        let label = def
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or(key)
            .to_string();
        if key == "title" {
            cols.insert(0, (key.as_str(), label));
        } else {
            cols.push((key.as_str(), label));
        }
    }
    if cols.is_empty() {
        return String::new();
    }
    let mut rows: Vec<&&serde_json::Value> = doc
        .blocks
        .values()
        .filter(|b| b.get("parent_id").and_then(|p| p.as_str()) == Some(collection_id))
        .collect();
    // HashMap iteration order is arbitrary; sort by the row title so repeated
    // fetches of the same page render identically (cache-stable output).
    rows.sort_by_key(|b| notion_text(b.get("properties").and_then(|p| p.get("title"))));
    let mut out = String::new();
    if !name.is_empty() {
        out.push_str(&format!("**{name}** (database, {} rows)\n\n", rows.len()));
    }
    let header: Vec<&str> = cols.iter().map(|(_, l)| l.as_str()).collect();
    out.push_str(&format!("| {} |\n", header.join(" | ")));
    out.push_str(&format!("|{}|\n", vec![" --- "; cols.len()].join("|")));
    for row in rows.iter().take(NOTION_ROWS_CAP) {
        let props = row.get("properties");
        let cells: Vec<String> = cols
            .iter()
            .map(|(k, _)| {
                notion_text(props.and_then(|p| p.get(*k)))
                    .replace('|', "\\|")
                    .replace('\n', " ")
            })
            .collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    if rows.len() > NOTION_ROWS_CAP {
        out.push_str(&format!("\n[{} more rows]\n", rows.len() - NOTION_ROWS_CAP));
    }
    out
}

/// Media/link blocks: a URL line carrying the caption, so the agent can see
/// what was embedded and follow it if the prose points there.
pub(super) fn notion_media_line(b: &serde_json::Value, ty: &str) -> String {
    let props = b.get("properties");
    let src = notion_text(props.and_then(|p| p.get("source")));
    let link = notion_text(props.and_then(|p| p.get("link")));
    let caption = notion_text(props.and_then(|p| p.get("caption")));
    let title = notion_text(props.and_then(|p| p.get("title")));
    let url = if !src.is_empty() { src } else { link };
    let label = [title, caption]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    match (url.is_empty(), label.is_empty()) {
        (true, true) => String::new(),
        (true, false) => label,
        (false, true) => format!("[{ty}] <{url}>"),
        (false, false) => format!("[{ty}] {label} <{url}>"),
    }
}

pub(super) fn notion_walk(
    doc: &NotionDoc<'_>,
    id: &str,
    depth: usize,
    out: &mut String,
    seen: &mut std::collections::HashSet<String>,
) {
    if out.len() >= NOTION_MAX_CHARS || depth > NOTION_MAX_DEPTH {
        return;
    }
    // Synced blocks and column containers can reference an ANCESTOR. Those
    // types emit no line and do not increment depth, so a cycle among them
    // grows `out` by zero bytes — the size cap never fires and the recursion
    // overflows the stack, which aborts the process rather than panicking
    // (uncatchable, and this is remote server data). Visit each block once.
    if !seen.insert(id.to_string()) {
        return;
    }
    let Some(b) = doc.blocks.get(id) else { return };
    let ty = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let props = b.get("properties");
    let text = notion_text(props.and_then(|p| p.get("title")));
    let indent = "  ".repeat(depth.saturating_sub(1));
    let line = match ty {
        "page" if depth == 0 => format!("# {text}"),
        // A nested page is a link to a document we are not fetching; flag it
        // as such rather than pretending its body is here.
        "page" => format!("{indent}- [sub-page] {text}"),
        "header" => format!("## {text}"),
        "sub_header" => format!("### {text}"),
        "sub_sub_header" => format!("#### {text}"),
        "bulleted_list" | "toggle" => format!("{indent}- {text}"),
        "numbered_list" => format!("{indent}1. {text}"),
        "to_do" => {
            let done = props
                .and_then(|p| p.get("checked"))
                .map(|c| notion_text(Some(c)) == "Yes")
                .unwrap_or(false);
            format!("{indent}- [{}] {text}", if done { "x" } else { " " })
        }
        "quote" | "callout" => format!("> {text}"),
        "code" => {
            let lang = notion_text(props.and_then(|p| p.get("language"))).to_lowercase();
            format!("```{lang}\n{text}\n```")
        }
        "equation" => format!("$${text}$$"),
        "divider" => "---".to_string(),
        "image" | "video" | "audio" | "file" | "pdf" | "embed" | "bookmark" => {
            let mut l = notion_media_line(b, ty);
            // A bookmark's description is the only prose it carries.
            let desc = notion_text(props.and_then(|p| p.get("description")));
            if !desc.is_empty() {
                l.push_str(&format!("\n{desc}"));
            }
            l
        }
        "collection_view" | "collection_view_page" => b
            .get("collection_id")
            .and_then(|c| c.as_str())
            .map(|cid| notion_collection_table(doc, cid))
            .unwrap_or_default(),
        // Pure containers (columns, synced blocks): no line of their own,
        // children carry everything. Depth is not incremented for them so a
        // two-column layout doesn't indent its prose.
        "column_list" | "column" | "transclusion_container" | "transclusion_reference" => {
            String::new()
        }
        // Navigation chrome with no content: a ToC restates headings we
        // already render, a breadcrumb restates the page title.
        "table_of_contents" | "breadcrumb" | "button" => String::new(),
        _ if text.is_empty() => String::new(),
        _ => format!("{indent}{text}"),
    };
    if !line.trim().is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(line.trim_end());
    }
    let child_depth = match ty {
        "column_list" | "column" | "transclusion_container" | "transclusion_reference" => depth,
        _ => depth + 1,
    };
    if let Some(content) = b.get("content").and_then(|c| c.as_array()) {
        for child in content.iter().filter_map(|c| c.as_str()) {
            notion_walk(doc, child, child_depth, out, seen);
        }
    }
}

/// Render the concatenated Notion payloads (`loadPageChunk` responses, then
/// `queryCollection` responses, joined by [`AUX_SEP`]). Order-independent:
/// every part contributes its `recordMap` entries to one merged document.
///
/// A private/deleted page answers with an error object carrying no
/// `recordMap`, so this returns `None` and the caller falls back to the
/// generic pipeline.
pub(super) fn notion_page(body: &str) -> Option<String> {
    // The parsed `Value`s must outlive the borrowed maps built from them.
    let parts: Vec<serde_json::Value> = body
        .split(AUX_SEP)
        .filter_map(|p| serde_json::from_str(p).ok())
        .collect();
    let mut blocks: NotionBlocks<'_> = Default::default();
    let mut collections: NotionCollections<'_> = Default::default();
    for v in &parts {
        let Some(rm) = v.get("recordMap") else {
            continue;
        };
        // `recordMap.<table>[id].value.value` — the outer `value` is the
        // space/role envelope, the inner one the record itself.
        for (table, sink) in [("block", &mut blocks), ("collection", &mut collections)] {
            if let Some(obj) = rm.get(table).and_then(|b| b.as_object()) {
                for (k, v) in obj {
                    if let Some(inner) = v.get("value").and_then(|v| v.get("value")) {
                        sink.insert(k.as_str(), inner);
                    }
                }
            }
        }
    }
    let doc = NotionDoc {
        blocks,
        collections,
    };
    // The requested page id, pinned by the fetcher via [`notion_root_part`].
    // Without it the heuristic below can pick the WRONG page: `queryCollection`
    // drags in the workspace's parent pages, which are `page` blocks that
    // nothing in the payload lists as a child either (MEASURED — a page came
    // back rendered as its parent wiki's front page).
    let pinned = parts
        .iter()
        .find_map(|p| p.get("tokenadePageId").and_then(|v| v.as_str()))
        .filter(|id| doc.blocks.contains_key(id));
    // Fallback: the only `page` block no other block lists as a child.
    // Database rows are `page` blocks too, but they are parented to their
    // collection rather than listed as content — hence the `parent_table`
    // check, without which a row could be mistaken for the page itself.
    let children: std::collections::HashSet<&str> = doc
        .blocks
        .values()
        .filter_map(|b| b.get("content")?.as_array())
        .flatten()
        .filter_map(|c| c.as_str())
        .collect();
    let root = match pinned {
        Some(id) => id,
        None => {
            *doc.blocks
                .iter()
                .find(|(id, b)| {
                    !children.contains(*id)
                        && b.get("type").and_then(|t| t.as_str()) == Some("page")
                        && b.get("parent_table").and_then(|t| t.as_str()) != Some("collection")
                })?
                .0
        }
    };
    let mut out = String::new();
    let mut seen = std::collections::HashSet::new();
    notion_walk(&doc, root, 0, &mut out, &mut seen);
    if out.len() >= NOTION_MAX_CHARS {
        out.push_str("\n\n[tokenade: page truncated]");
    }
    (!out.trim().is_empty()).then_some(out)
}

/// Endpoint every Notion follow-up fetch targets. `notion.site` custom
/// domains proxy the same API, but `www.notion.so` answers for every public
/// page id regardless of which domain published it (MEASURED).
pub const NOTION_API: &str = "https://www.notion.so/api/v3/";

/// A rendered Notion page is cut here so a 500-block wiki can't blow the
/// inline budget.
pub(crate) const NOTION_MAX_CHARS: usize = 40_000;
/// Rows fetched per embedded database, and rendered per table.
pub(crate) const NOTION_ROWS_CAP: usize = 50;
/// Chunks fetched per page. 100 blocks each — well past any page whose prose
/// still fits an agent's context.
pub const NOTION_MAX_CHUNKS: usize = 6;

/// Deepest block nesting we will follow. Belt-and-braces behind the visited
/// set: a pathological but acyclic tree must not eat the stack either.
pub(crate) const NOTION_MAX_DEPTH: usize = 64;
