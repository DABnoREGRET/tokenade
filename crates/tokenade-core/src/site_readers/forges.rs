//! GitHub and GitLab, read through their APIs.
//!
//! A `blob` page is a JavaScript code viewer, an issue page distils to shell,
//! and an Actions run drops the step that failed — the part worth fetching for.
//! The v3/v4 APIs carry all of it unauthenticated.

use super::*;

/// Fence language for a source file, by extension. Unknown extensions get a
/// bare fence rather than a wrong highlight.
///
/// Resolved when the PLAN is built, because that is the only point where the
/// file path is still in scope: [`render`] sees a body and a [`Kind`] and
/// nothing else, which is why the blob reader shipped bare fences until the
/// language rode along inside `Kind::GithubBlob`.
pub(super) fn fence_lang(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "rb" => "ruby",
        "php" => "php",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "cs" => "csharp",
        "swift" => "swift",
        "sh" | "bash" => "bash",
        "sql" => "sql",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "json" => "json",
        "md" => "markdown",
        "html" => "html",
        "css" => "css",
        "xml" => "xml",
        _ => "",
    }
}

/// A raw source file, fenced. The fetcher passes the file bytes through
/// untouched, so this only frames them; the downstream inline cap decides
/// how much of a large file is actually served.
pub(super) fn github_blob(body: &str, lang: &str) -> Option<String> {
    if body.trim().is_empty() {
        return None;
    }
    // raw.githubusercontent answers "404: Not Found" as a plain-text body.
    if body.trim_start().starts_with("404: Not Found") {
        return None;
    }
    Some(format!("```{lang}\n{}\n```", body.trim_end()))
}

/// Render an Actions run: what ran, whether it passed, and — the reason
/// anyone opens this page — which step failed.
pub(super) fn github_actions_run(body: &str) -> Option<String> {
    let (head, jobs_raw) = match body.split_once(AUX_SEP) {
        Some((h, j)) => (h, j),
        None => (body, ""),
    };
    let v: serde_json::Value = serde_json::from_str(head.trim()).ok()?;
    let name = v.get("name").and_then(|n| n.as_str())?;
    let st = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (status, conclusion, branch, event) = (
        st("status"),
        st("conclusion"),
        st("head_branch"),
        st("event"),
    );
    let number = v.get("run_number").and_then(|n| n.as_u64()).unwrap_or(0);
    let mut out = format!(
        "# {name} · run #{number}\n\n{status}{} · on {branch} · triggered by {event}",
        if conclusion.is_empty() {
            String::new()
        } else {
            format!(" ({conclusion})")
        }
    );
    if let Some(t) = v
        .get("head_commit")
        .and_then(|c| c.get("message"))
        .and_then(|m| m.as_str())
    {
        out.push_str(&format!("\n\ncommit: {}", t.lines().next().unwrap_or("")));
    }
    let Ok(jobs) = serde_json::from_str::<serde_json::Value>(jobs_raw.trim()) else {
        return Some(out);
    };
    let Some(list) = jobs.get("jobs").and_then(|j| j.as_array()) else {
        return Some(out);
    };
    out.push_str("\n\n## Jobs\n");
    for j in list {
        let jn = j.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        let jc = j
            .get("conclusion")
            .and_then(|c| c.as_str())
            .unwrap_or("running");
        out.push_str(&format!("\n- **{jn}** — {jc}"));
        // Only failed steps are worth the tokens; a green run's step list is
        // noise, and a red one is the whole reason for the fetch.
        if jc == "failure" {
            if let Some(steps) = j.get("steps").and_then(|s| s.as_array()) {
                for s in steps {
                    let sc = s.get("conclusion").and_then(|c| c.as_str()).unwrap_or("");
                    if sc == "failure" {
                        let sn = s.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        out.push_str(&format!("\n  - FAILED step: {sn}"));
                    }
                }
            }
        }
    }
    Some(out)
}

/// Render a GitLab issue or merge request. Comments are NOT included: the
/// v4 `/notes` endpoint answers 401 without a token, so the thread body is
/// all a public reader can honestly serve.
pub(super) fn gitlab_thread(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let title = v.get("title")?.as_str()?;
    let iid = v.get("iid").and_then(|i| i.as_u64()).unwrap_or(0);
    let state = v.get("state").and_then(|s| s.as_str()).unwrap_or("");
    let author = v
        .get("author")
        .and_then(|a| a.get("username"))
        .and_then(|u| u.as_str())
        .unwrap_or("?");
    // MRs carry source/target branches; issues do not.
    let what = if v.get("source_branch").is_some() {
        "MR"
    } else {
        "Issue"
    };
    let mut out = format!("# {title}\n\n{what} !{iid} · {state} · opened by @{author}");
    if let Some(labels) = v.get("labels").and_then(|l| l.as_array()) {
        let names: Vec<&str> = labels.iter().filter_map(|l| l.as_str()).collect();
        if !names.is_empty() {
            out.push_str(&format!(" · labels: {}", names.join(", ")));
        }
    }
    if let (Some(src), Some(dst)) = (
        v.get("source_branch").and_then(|b| b.as_str()),
        v.get("target_branch").and_then(|b| b.as_str()),
    ) {
        out.push_str(&format!("\n\n{src} → {dst}"));
    }
    if let Some(d) = v.get("description").and_then(|d| d.as_str()) {
        if !d.trim().is_empty() {
            out.push_str(&format!("\n\n{}", d.trim()));
        }
    }
    let n = v
        .get("user_notes_count")
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    if n > 0 {
        out.push_str(&format!(
            "\n\n[{n} comments not shown — GitLab's notes API requires a token]"
        ));
    }
    Some(out)
}

/// Render the file list of a PR (`/pulls/{n}/files`) as a changed-files
/// table. The reader appends it to the thread: an agent opening a PR wants
/// the CHANGE, and the issue endpoint carries only the discussion.
///
/// Deliberately the file list and not the raw diff: `.diff` on a large PR is
/// tens of thousands of tokens the agent rarely needs whole, while filenames
/// with their +/- counts tell it where to look and cost ~10 lines.
pub fn github_pr_files(body: &str) -> Option<String> {
    let list = serde_json::from_str::<serde_json::Value>(body.trim()).ok()?;
    let list = list.as_array()?;
    if list.is_empty() {
        return None;
    }
    let mut out = String::from("\n\n## Files changed\n");
    let (mut adds, mut dels) = (0u64, 0u64);
    for f in list.iter().take(50) {
        let name = f.get("filename").and_then(|n| n.as_str()).unwrap_or("?");
        let a = f.get("additions").and_then(|n| n.as_u64()).unwrap_or(0);
        let d = f.get("deletions").and_then(|n| n.as_u64()).unwrap_or(0);
        let status = f.get("status").and_then(|s| s.as_str()).unwrap_or("");
        adds += a;
        dels += d;
        out.push_str(&format!(
            "\n- `{name}` +{a}/-{d}{}",
            if status == "modified" {
                String::new()
            } else {
                format!(" ({status})")
            }
        ));
    }
    if list.len() > 50 {
        out.push_str(&format!("\n- […{} more files]", list.len() - 50));
    }
    out.push_str(&format!("\n\n{} files · +{adds}/-{dels}", list.len()));
    Some(out)
}

/// Render a GitHub issue/PR: the API object, then the comments array after
/// [`AUX_SEP`]. `None` on a rate-limit / not-found body (no `title`), which
/// sends the caller back to the generic pipeline.
pub(super) fn github_issue(body: &str) -> Option<String> {
    let (head, comments_raw) = match body.split_once(AUX_SEP) {
        Some((h, c)) => (h, c),
        None => (body, ""),
    };
    let v: serde_json::Value = serde_json::from_str(head.trim()).ok()?;
    let title = v.get("title")?.as_str()?;
    let number = v.get("number").and_then(|n| n.as_u64()).unwrap_or(0);
    // `pull_request` is present only on PRs — the API serves both here.
    let what = if v.get("pull_request").is_some() {
        "PR"
    } else {
        "Issue"
    };
    let state = v.get("state").and_then(|s| s.as_str()).unwrap_or("");
    let author = v
        .get("user")
        .and_then(|u| u.get("login"))
        .and_then(|l| l.as_str())
        .unwrap_or("?");
    let mut out = format!("# {title}\n\n{what} #{number} · {state} · opened by @{author}");
    if let Some(labels) = v.get("labels").and_then(|l| l.as_array()) {
        let names: Vec<&str> = labels
            .iter()
            .filter_map(|l| l.get("name").and_then(|n| n.as_str()))
            .collect();
        if !names.is_empty() {
            out.push_str(&format!(" · labels: {}", names.join(", ")));
        }
    }
    if let Some(b) = v.get("body").and_then(|b| b.as_str()) {
        if !b.trim().is_empty() {
            out.push_str(&format!("\n\n{}", b.trim()));
        }
    }
    let Ok(comments) = serde_json::from_str::<serde_json::Value>(comments_raw.trim()) else {
        return Some(out);
    };
    let Some(list) = comments.as_array() else {
        return Some(out);
    };
    if list.is_empty() {
        return Some(out);
    }
    let total = v
        .get("comments")
        .and_then(|c| c.as_u64())
        .unwrap_or(list.len() as u64);
    out.push_str(&format!("\n\n## Comments ({total})\n"));
    let mut written = 0usize;
    let mut shown = 0usize;
    for c in list {
        let who = c
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or("?");
        let text = c.get("body").and_then(|b| b.as_str()).unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let block = format!("\n**@{who}**\n{text}\n");
        if written + block.len() > GITHUB_COMMENTS_CAP_CHARS {
            break;
        }
        written += block.len();
        shown += 1;
        out.push_str(&block);
    }
    if (total as usize) > shown {
        out.push_str(&format!("\n[{} more comments]\n", total as usize - shown));
    }
    Some(out)
}

/// Render a GitHub issue/PR search result set as the list the site's own page
/// refuses to server-render.
pub(super) fn github_issue_search(body: &str) -> Option<String> {
    const CAP_CHARS: usize = 16_000;
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    // A rate-limited or malformed query answers `{"message":…}` with no
    // `items`, which fails here and routes the caller back to the generic page.
    let items = v.get("items")?.as_array()?;
    // An empty result set is where a reader can only lose. The site's filter
    // language is a superset of the search API's, so a qualifier the API does
    // not know comes back as zero matches rather than an error — and
    // "0 matching" served over a page that really does list issues would be a
    // reader making the answer WORSE. Hand those back to the generic pipeline;
    // a genuinely empty list costs nothing there either.
    if items.is_empty() {
        return None;
    }
    let total = v
        .get("total_count")
        .and_then(|t| t.as_u64())
        .unwrap_or(items.len() as u64);
    let mut out = format!(
        "# {total} matching issue(s)/pull request(s)\n> showing {} — api.github.com/search\n",
        items.len()
    );
    let mut len = 0usize;
    let mut dropped = 0usize;
    for it in items {
        if len > CAP_CHARS {
            dropped += 1;
            continue;
        }
        let (Some(num), Some(title)) = (
            it.get("number").and_then(|n| n.as_u64()),
            it.get("title").and_then(|t| t.as_str()),
        ) else {
            continue;
        };
        let mut facts: Vec<String> = Vec::new();
        // `state` alone calls a merged PR "closed"; the site shows the two
        // differently and so does anyone reading this list.
        let merged = it
            .pointer("/pull_request/merged_at")
            .is_some_and(|m| !m.is_null());
        facts.push(match it.get("state").and_then(|s| s.as_str()) {
            Some("closed") if merged => "merged".into(),
            Some(s) => s.to_string(),
            None => "?".into(),
        });
        if it.pointer("/user/login").is_some() {
            facts.push(format!(
                "by {}",
                it.pointer("/user/login")?.as_str().unwrap_or("?")
            ));
        }
        if let Some(c) = it
            .get("comments")
            .and_then(|c| c.as_u64())
            .filter(|c| *c > 0)
        {
            facts.push(format!("{c} comments"));
        }
        if let Some(d) = it.get("updated_at").and_then(|d| d.as_str()) {
            facts.push(format!("updated {}", d.split('T').next().unwrap_or(d)));
        }
        let labels: Vec<&str> = it
            .get("labels")
            .and_then(|l| l.as_array())
            .into_iter()
            .flatten()
            .filter_map(|l| l.get("name").and_then(|n| n.as_str()))
            .collect();
        if !labels.is_empty() {
            facts.push(labels.join(", "));
        }
        let mut entry = format!("\n## #{num} {}\n{}\n", title.trim(), facts.join(" • "));
        if let Some(u) = it.get("html_url").and_then(|u| u.as_str()) {
            entry.push_str(&format!("{u}\n"));
        }
        len += entry.chars().count();
        out.push_str(&entry);
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n({dropped} further result(s) beyond the size cap not shown)\n"
        ));
    }
    Some(out.trim_end().to_string())
}

/// The first path segments GitHub reserves for itself: they are not
/// accounts, and `/{owner}/{repo}` would catch them all.
const GITHUB_RESERVED_OWNERS: [&str; 24] = [
    "features",
    "about",
    "pricing",
    "login",
    "logout",
    "join",
    "settings",
    "marketplace",
    "explore",
    "topics",
    "collections",
    "sponsors",
    "orgs",
    "enterprise",
    "notifications",
    "codespaces",
    "search",
    "new",
    "apps",
    "site",
    "security",
    "readme",
    "contact",
    "customer-stories",
];

/// **A repository's home page had no reader**, and it is the GitHub address an
/// agent asks for most.
///
/// MEASURED 2026-09-12 on `github.com/rust-lang/regex`: the delivered page
/// gives the title, the description, **twice "Uh oh! There was an error while
/// loading. Please reload this page."**, the Fork/Star/Notifications buttons
/// and a commit count — and **not one line of README**, which is rendered
/// client-side. Field telemetry flagged `github.com` as "write a reader" over
/// five reads and 117 extracted tokens on average.
///
/// The public API carries the card, and `raw.githubusercontent.com` the README
/// at the default revision — `HEAD` is a valid ref there, so no third call is
/// needed to read `default_branch`.
pub(super) fn github_repo_plan(path: &str) -> Option<Plan> {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    let c = re(r"^/([^/]+)/([^/]+?)(?:\.git)?/?$").captures(clean)?;
    let (owner, repo) = (c.get(1)?.as_str(), c.get(2)?.as_str());
    if GITHUB_RESERVED_OWNERS.contains(&owner.to_ascii_lowercase().as_str()) {
        return None;
    }
    Some(
        Plan::get(
            format!("https://api.github.com/repos/{owner}/{repo}"),
            Kind::GithubRepo,
            "api.github.com",
        )
        .with_aux(format!(
            "https://raw.githubusercontent.com/{owner}/{repo}/HEAD/README.md"
        )),
    )
}

/// A repository's README is markdown already written; anything past this is
/// cut.
const GITHUB_README_MAX_CHARS: usize = 20_000;

/// Renders a repository card: what the API says, then the README.
pub(super) fn github_repo(body: &str) -> Option<String> {
    let (head, readme) = match body.split_once(AUX_SEP) {
        Some((h, r)) => (h, r),
        None => (body, ""),
    };
    let v: serde_json::Value = serde_json::from_str(head.trim()).ok()?;
    // `full_name` is what EVERY card carries. Its absence is the API's error
    // response (`{"message":"Not Found"}`, or the rate limit), and rendering
    // that as a repository would serve an error message as content.
    let full_name = v.get("full_name").and_then(|n| n.as_str())?;
    let st = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let mut out = format!("# {full_name}");
    if v.get("archived").and_then(|a| a.as_bool()) == Some(true) {
        out.push_str(" (archived)");
    }
    let desc = st("description");
    if !desc.is_empty() {
        out.push_str(&format!("\n\n> {desc}"));
    }
    let mut facts = vec![format!("{} stars", n("stargazers_count"))];
    facts.push(format!("{} forks", n("forks_count")));
    facts.push(format!("{} open issues", n("open_issues_count")));
    let lang = st("language");
    if !lang.is_empty() {
        facts.push(lang);
    }
    if let Some(l) = v
        .get("license")
        .and_then(|l| l.get("spdx_id"))
        .and_then(|l| l.as_str())
        .filter(|l| *l != "NOASSERTION")
    {
        facts.push(l.to_string());
    }
    let pushed = st("pushed_at");
    if !pushed.is_empty() {
        facts.push(format!("last push {pushed}"));
    }
    out.push_str(&format!("\n\n{}", facts.join(" · ")));
    let home = st("homepage");
    if !home.is_empty() {
        out.push_str(&format!("\n\nhomepage: <{home}>"));
    }
    if let Some(topics) = v.get("topics").and_then(|t| t.as_array()) {
        let list: Vec<&str> = topics.iter().filter_map(|t| t.as_str()).collect();
        if !list.is_empty() {
            out.push_str(&format!("\n\ntopics: {}", list.join(", ")));
        }
    }
    // `raw` answers "404: Not Found" as PLAIN TEXT when the file does not
    // exist: a repository without `README.md` (or whose README has another
    // name) would deliver that line as if it had written it.
    let readme = readme.trim();
    if !readme.is_empty() && !readme.starts_with("404: Not Found") {
        out.push_str("\n\n## README\n\n");
        if readme.chars().count() > GITHUB_README_MAX_CHARS {
            let kept: String = readme.chars().take(GITHUB_README_MAX_CHARS).collect();
            let left = readme.chars().count() - GITHUB_README_MAX_CHARS;
            out.push_str(&kept);
            out.push_str(&format!(
                "\n\n[README cut here — {left} more characters at \
                 <https://github.com/{full_name}>]"
            ));
        } else {
            out.push_str(readme);
        }
    }
    Some(out)
}

pub(super) fn github_blob_plan(path: &str) -> Option<Plan> {
    // Strip query/fragment like every other arm: `#L10-L20` and
    // `?plain=1` are the most-shared blob URL shapes and both 404 on
    // raw.githubusercontent.
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    let c = re(r"^/([^/]+)/([^/]+)/blob/([^/]+)/(.+)$").captures(clean)?;
    let (owner, repo, git_ref, file) = (
        c.get(1)?.as_str(),
        c.get(2)?.as_str(),
        c.get(3)?.as_str(),
        c.get(4)?.as_str(),
    );
    Some(Plan::get(
        format!("https://raw.githubusercontent.com/{owner}/{repo}/{git_ref}/{file}"),
        Kind::GithubBlob(fence_lang(file)),
        "raw.githubusercontent.com",
    ))
}

pub(super) fn github_actions_run_plan(path: &str) -> Option<Plan> {
    let c = re(r"^/([^/]+)/([^/]+)/actions/runs/(\d+)").captures(path)?;
    let (owner, repo, id) = (c.get(1)?.as_str(), c.get(2)?.as_str(), c.get(3)?.as_str());
    Some(
        Plan::get(
            format!("https://api.github.com/repos/{owner}/{repo}/actions/runs/{id}"),
            Kind::GithubActionsRun,
            "api.github.com",
        )
        .with_aux(format!(
            "https://api.github.com/repos/{owner}/{repo}/actions/runs/{id}/jobs"
        )),
    )
}

pub(super) fn gitlab_issue_plan(path: &str) -> Option<Plan> {
    let c = re(r"^/(.+?)/-/(issues|merge_requests)/(\d+)").captures(path)?;
    let (proj, kind_seg, iid) = (c.get(1)?.as_str(), c.get(2)?.as_str(), c.get(3)?.as_str());
    // The API keys projects by their URL-encoded full path.
    let encoded = proj.replace('/', "%2F");
    Some(Plan::get(
        format!("https://gitlab.com/api/v4/projects/{encoded}/{kind_seg}/{iid}"),
        Kind::GitlabThread,
        "gitlab.com/api/v4",
    ))
}

pub(super) fn github_issue_plan(path: &str) -> Option<Plan> {
    let c = re(r"^/([^/]+)/([^/]+)/(?:issues|pull)/(\d+)").captures(path)?;
    let (owner, repo, num) = (c.get(1)?.as_str(), c.get(2)?.as_str(), c.get(3)?.as_str());
    // A PR IS an issue in this API, so one endpoint serves both.
    Some(
        Plan::get(
            format!("https://api.github.com/repos/{owner}/{repo}/issues/{num}"),
            Kind::GithubIssue,
            "api.github.com",
        )
        .with_aux(format!(
            "https://api.github.com/repos/{owner}/{repo}/issues/{num}/comments?per_page=30"
        )),
    )
}

pub(super) fn github_issue_list_plan(path: &str) -> Option<Plan> {
    let c = re(r"^/([^/]+)/([^/]+)/(issues|pulls)").captures(path)?;
    let (owner, repo, seg) = (c.get(1)?.as_str(), c.get(2)?.as_str(), c.get(3)?.as_str());
    let kind_q = if seg == "pulls" {
        "is%3Apr"
    } else {
        "is%3Aissue"
    };
    // The `q=` value is already percent-encoded for a URL, and the API
    // takes the same encoding — forward it rather than round-tripping.
    let user_q = re(r"[?&]q=([^&#]*)")
        .captures(path)
        .map(|c| c[1].to_string());
    let q = match user_q.filter(|q| !q.trim().is_empty()) {
        // A repo-scoped page whose filter names its own repo would AND
        // two `repo:` qualifiers and match nothing, so only add ours
        // when the filter has not already pinned one.
        Some(q) if q.contains("repo%3A") || q.contains("repo:") => q,
        Some(q) => format!("repo%3A{owner}%2F{repo}+{q}"),
        // GitHub's unfiltered list is implicitly open-only.
        None => format!("repo%3A{owner}%2F{repo}+{kind_q}+is%3Aopen"),
    };
    Some(Plan::get(
        format!("https://api.github.com/search/issues?q={q}&per_page=30"),
        Kind::GithubIssueSearch,
        "api.github.com/search",
    ))
}

/// Comment sections are capped so a 255-reply issue can't blow the budget.
pub(crate) const GITHUB_COMMENTS_CAP_CHARS: usize = 20_000;
