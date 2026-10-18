//! **A PDF link covers only ITS text.**
//!
//! `pdf_oxide` links a span as soon as its box TOUCHES the rectangle of a
//! `/Link` annotation (`apply_link_annotations_to_ordered_spans`, 0.3.74 and
//! 0.3.77): the neighbouring word on the line and the line below, whose box
//! grazes the rectangle, received the address. An `.odt` exported to PDF read
//! `[Web](…)[example site](…)` et `[Note here](https://example.com/a…)1[end.](…)`
//! — links the document does not set (2026-10-04).
//!
//! This module redoes the measurement: the text a rectangle REALLY covers (span
//! whose vertical centre is inside, characters whose estimated horizontal centre
//! falls in it), and a `[text](address)` fragment none of whose words is
//! in that text becomes text again. With no measurement possible (no span under the
//! rectangle), nothing is removed.

use std::collections::HashMap;

/// A `/Link` annotation to an address: rectangle (x0, y0, x1, y1).
pub(super) struct Area {
    pub rect: [f32; 4],
    pub uri: String,
}

/// A span: text, x, y (bottom), width, height.
pub(super) type Span = (String, f32, f32, f32, f32);

pub(super) fn link_areas(doc: &pdf_oxide::PdfDocument, page: usize) -> Vec<Area> {
    doc.get_annotations(page)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| {
            let uri = match a.action? {
                pdf_oxide::LinkAction::Uri(u) if !u.is_empty() => u,
                _ => return None,
            };
            let r = a.rect?;
            let rect = [
                r[0].min(r[2]) as f32,
                r[1].min(r[3]) as f32,
                r[0].max(r[2]) as f32,
                r[1].max(r[3]) as f32,
            ];
            Some(Area { rect, uri })
        })
        .collect()
}

fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

/// The words the rectangle really covers.
fn covered(rect: [f32; 4], spans: &[Span]) -> String {
    let mut out = String::new();
    for (text, x, y, w, h) in spans {
        let mid = y + h / 2.0;
        if mid < rect[1] || mid > rect[3] {
            continue;
        }
        let n = text.chars().count().max(1) as f32;
        for (i, c) in text.chars().enumerate() {
            let cx = x + w * (i as f32 + 0.5) / n;
            out.push(if cx >= rect[0] && cx <= rect[2] {
                c
            } else {
                ' '
            });
        }
        out.push(' ');
    }
    out
}

/// The markdown of a page, each link set outside its rectangle turned back into
/// text.
pub(super) fn without_misplaced_links(md: &str, areas: &[Area], spans: &[Span]) -> String {
    if areas.is_empty() {
        return md.to_string();
    }
    let mut truth: HashMap<&str, std::collections::HashSet<String>> = HashMap::new();
    for a in areas {
        truth
            .entry(a.uri.as_str())
            .or_default()
            .extend(words(&covered(a.rect, spans)));
    }
    if truth.values().all(|k| k.is_empty()) {
        return md.to_string();
    }
    // The library writes each linked fragment WITHOUT the space separating it
    // from its neighbours (`See[the source](…)`, `Note here1end.` once unlinked):
    // the space comes back where a word touches the fragment.
    let link = super::super::markup_marks::re(r"\[([^\]\n]*)\]\(([^)\s]+)\)");
    let mut out = String::with_capacity(md.len());
    let mut last = 0;
    for c in link.captures_iter(md) {
        let (Some(whole), Some(text), Some(uri)) = (c.get(0), c.get(1), c.get(2)) else {
            continue;
        };
        if !truth.contains_key(uri.as_str()) {
            continue;
        }
        out.push_str(&md[last..whole.start()]);
        let misplaced = truth
            .get(uri.as_str())
            .is_some_and(|k| !k.is_empty() && !words(text.as_str()).any(|w| k.contains(&w)));
        let piece = if misplaced {
            text.as_str()
        } else {
            whole.as_str()
        };
        // No space before punctuation (`, mail` unlinked after a link).
        let opens_a_word = piece
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '[');
        if opens_a_word
            && out
                .chars()
                .last()
                .is_some_and(|p| p.is_alphanumeric() || matches!(p, ')' | '.' | ',' | ';' | ':'))
        {
            out.push(' ');
        }
        out.push_str(piece);
        if md[whole.end()..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric)
        {
            out.push(' ');
        }
        last = whole.end();
    }
    out.push_str(&md[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fragment_outside_its_rectangle_loses_the_link() {
        // « Web example site, mail » on one line; « Note here 1 end. » below.
        // The link covers « example site » (x 30..90), the box of the line
        // below grazes the rectangle from above.
        let spans: Vec<Span> = vec![
            ("Web ".into(), 0.0, 100.0, 24.0, 12.0),
            ("example site".into(), 30.0, 100.0, 60.0, 12.0),
            (", mail".into(), 90.0, 100.0, 30.0, 12.0),
            ("Note here".into(), 0.0, 86.0, 50.0, 15.0),
        ];
        let areas = vec![Area {
            rect: [29.0, 98.0, 91.0, 113.0],
            uri: "https://e.x/a".into(),
        }];
        let md = "[Web](https://e.x/a)[example site](https://e.x/a)[, mail](https://e.x/a)\n[Note here](https://e.x/a)1 [end.](https://e.x/a) and [other](https://o.x/)";
        assert_eq!(
            without_misplaced_links(md, &areas, &spans),
            "Web [example site](https://e.x/a), mail\nNote here 1 end. and [other](https://o.x/)"
        );
        // Nothing under the rectangle: no measurement, nothing is removed.
        assert_eq!(without_misplaced_links(md, &areas, &[]), md);
    }
}
