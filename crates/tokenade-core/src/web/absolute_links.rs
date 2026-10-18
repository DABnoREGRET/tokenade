//! **A link the agent can open.** The page wrote its links relative to
//! itself; once only the markdown is left, `](../x.html)` names nothing.
//!
//! Resolved, but not spelled out in full: the agent asked for the page, so it
//! knows the host. Fully absolute links MEASURED 2026-10-05 at +8 % tokens on
//! docs.python.org `os.path` and +35 % on doc.rust-lang.org `Vec` (147 links,
//! 29 of them to `primitive.usize.html`) — for a prefix the reader already has.
//! So a link on the page's own host is written from the site root
//! (`/3/builtins/exceptions.html#OSError`: unambiguous, about the length of
//! the relative form), one into the page itself as its `#anchor`, and one to
//! another host in full.

/// Every markdown link or image destination in `md` resolved against
/// `page_url` (see the module doc for the form). Destinations with a scheme
/// or a bare `#anchor`, and anything inside a fenced code block, are left as
/// written; a page URL that does not parse leaves the whole text unchanged.
pub fn resolve_links(md: &str, page_url: &str) -> String {
    let Ok(base) = url::Url::parse(page_url) else {
        return md.to_string();
    };
    if !matches!(base.scheme(), "http" | "https") || !md.contains("](") {
        return md.to_string();
    }
    let mut out = String::with_capacity(md.len() + md.len() / 16);
    let mut in_fence = false;
    for (i, line) in md.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            in_fence = !in_fence;
        }
        if in_fence || !line.contains("](") {
            out.push_str(line);
            continue;
        }
        out.push_str(&resolve_line(line, &base));
    }
    out
}

fn resolve_line(line: &str, base: &url::Url) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("](") {
        out.push_str(&rest[..i + 2]);
        rest = &rest[i + 2..];
        // `](<dest with spaces>)` or `](dest)`.
        let (wrapped, end) = if let Some(r) = rest.strip_prefix('<') {
            match r.find('>') {
                Some(j) => (true, j + 1),
                None => continue,
            }
        } else {
            match rest.find([')', ' ']) {
                Some(j) => (false, j),
                None => continue,
            }
        };
        let dest = if wrapped { &rest[1..end] } else { &rest[..end] };
        match resolved(dest, base) {
            Some(abs) if wrapped => out.push_str(&format!("<{abs}>")),
            Some(abs) => out.push_str(&abs),
            None => out.push_str(&rest[..if wrapped { end + 1 } else { end }]),
        }
        rest = &rest[if wrapped { end + 1 } else { end }..];
    }
    out.push_str(rest);
    out
}

fn resolved(dest: &str, base: &url::Url) -> Option<String> {
    let has_scheme = dest.split_once(':').is_some_and(|(s, _)| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    });
    if dest.is_empty() || dest.starts_with('#') || has_scheme {
        return None;
    }
    let abs = base.join(dest).ok()?;
    if abs.host_str() != base.host_str()
        || abs.scheme() != base.scheme()
        || abs.port() != base.port()
    {
        return Some(abs.to_string());
    }
    if abs.path() == base.path() && abs.query() == base.query() {
        if let Some(f) = abs.fragment() {
            return Some(format!("#{f}"));
        }
    }
    let mut short = abs.path().to_string();
    if let Some(q) = abs.query() {
        short.push('?');
        short.push_str(q);
    }
    if let Some(f) = abs.fragment() {
        short.push('#');
        short.push_str(f);
    }
    Some(short)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://docs.python.org/3/library/os.path.html";

    #[test]
    fn a_relative_link_is_resolved_against_the_page() {
        let md = "Raises [OSError](../builtins/exceptions.html#OSError) and ![x](/static/a.png).";
        assert_eq!(
            resolve_links(md, PAGE),
            "Raises [OSError](/3/builtins/exceptions.html#OSError) and ![x](/static/a.png)."
        );
    }

    #[test]
    fn what_is_already_absolute_or_in_page_is_left_alone() {
        let md = "[a](https://x.org/a) [b](mailto:a@b.c) [c](#top) [d](//cdn.x.org/d.js) [e](<my file.html>) [f](os.path.html#os.path.join)";
        assert_eq!(
            resolve_links(md, PAGE),
            "[a](https://x.org/a) [b](mailto:a@b.c) [c](#top) [d](https://cdn.x.org/d.js) [e](</3/library/my%20file.html>) [f](#os.path.join)"
        );
    }

    #[test]
    fn code_and_unparsable_pages_are_left_alone() {
        let md = "```\nx = a[0](rel)\n```\n[k](rel.html)";
        assert_eq!(
            resolve_links(md, PAGE),
            "```\nx = a[0](rel)\n```\n[k](/3/library/rel.html)"
        );
        assert_eq!(resolve_links("[k](rel.html)", "not a url"), "[k](rel.html)");
        assert_eq!(
            resolve_links("[k](rel.html)", "file:///tmp/a.html"),
            "[k](rel.html)"
        );
    }
}
