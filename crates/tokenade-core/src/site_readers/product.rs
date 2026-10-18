//! Product facts mined from a page's structured data.
//!
//! A shop page distils to navigation; its `schema.org/Product` block holds
//! the price, availability and rating an agent was asking for.

use super::*;

/// Product facts a page states about itself, as a markdown header.
///
/// Product pages are the worst case for generic extraction: allbirds.com
/// distils 213 735 tokens of HTML down to 630, ikea.com 283 360 down to 376 —
/// and neither of those remnants contains the price, which lives in metadata
/// the distiller correctly drops as chrome. So this does not REPLACE the page;
/// it prepends the facts the page declares, and the distilled body follows.
///
/// Two layers:
///   * JSON-LD `@type: Product` — the schema.org form Google's rich results
///     require. VERIFIED on woocommerce.com (name, 248 EUR, InStock, brand,
///     3.21/5 over 112 reviews, sku).
///   * OpenGraph `og:type=product` + `product:price:*` — VERIFIED standalone
///     on a live Shopify product page with its `ld+json` stripped out, which
///     still yielded name, `74 USD` and `instock`. The two layers COMPLEMENT
///     rather than duplicate: on that page JSON-LD carried brand and rating,
///     OpenGraph carried price and availability.
///
/// `og:type` is matched EXACTLY. IKEA labels its category listings
/// `product.group`, and a substring match served "Tous nos produits" as if it
/// were an article for sale.
///
/// `None` when the page does not declare itself a product, which leaves every
/// non-commerce page on the untouched generic path.
pub fn product_facts(html: &str) -> Option<String> {
    let ld = product_from_json_ld(html);
    let og_is_product = crate::web::meta_content(html, "og:type")
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("product"));
    if ld.is_none() && !og_is_product {
        return None;
    }
    let ld = ld.unwrap_or(serde_json::Value::Null);

    // JSON-LD wins per field; OpenGraph fills the gaps.
    let s = |v: &serde_json::Value, k: &str| -> Option<String> {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
    };
    let offers = match ld.get("offers") {
        Some(serde_json::Value::Array(a)) => a.first().cloned().unwrap_or(serde_json::Value::Null),
        Some(o) => o.clone(),
        None => serde_json::Value::Null,
    };
    let num_or_str = |v: &serde_json::Value, k: &str| -> Option<String> {
        match v.get(k) {
            Some(serde_json::Value::String(x)) if !x.trim().is_empty() => Some(x.trim().into()),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    };

    let name = s(&ld, "name")
        .or_else(|| crate::web::meta_content(html, "og:title"))
        .unwrap_or_else(|| "Product".into());
    let brand = ld
        .get("brand")
        .and_then(|b| {
            b.get("name")
                .and_then(|n| n.as_str())
                .or_else(|| b.as_str())
        })
        .map(|b| b.trim().to_string())
        .or_else(|| crate::web::meta_content(html, "product:brand"));
    let price = num_or_str(&offers, "price")
        .or_else(|| num_or_str(&offers, "lowPrice"))
        .or_else(|| crate::web::meta_content(html, "product:price:amount"))
        .or_else(|| crate::web::meta_content(html, "og:price:amount"));
    let currency = s(&offers, "priceCurrency")
        .or_else(|| crate::web::meta_content(html, "product:price:currency"))
        .or_else(|| crate::web::meta_content(html, "og:price:currency"));
    // schema.org spells availability as a URL (`…/schema.org/InStock`).
    let availability = s(&offers, "availability")
        .map(|a| a.rsplit('/').next().unwrap_or(&a).to_string())
        .or_else(|| crate::web::meta_content(html, "og:availability"))
        .or_else(|| crate::web::meta_content(html, "product:availability"));
    let sku = s(&ld, "sku").or_else(|| s(&ld, "mpn"));
    let rating = ld.get("aggregateRating").and_then(|r| {
        let v = num_or_str(r, "ratingValue")?;
        let n = num_or_str(r, "reviewCount").or_else(|| num_or_str(r, "ratingCount"));
        Some(match n {
            Some(n) => format!("{v}/5 ({n} reviews)"),
            None => format!("{v}/5"),
        })
    });

    let mut out = format!("# {name}\n");
    let mut facts: Vec<String> = Vec::new();
    if let (Some(p), Some(c)) = (&price, &currency) {
        facts.push(format!("**{p} {c}**"));
    } else if let Some(p) = &price {
        facts.push(format!("**{p}**"));
    }
    for f in [
        availability,
        brand.map(|b| format!("brand: {b}")),
        rating,
        sku.map(|s| format!("sku: {s}")),
    ]
    .into_iter()
    .flatten()
    {
        facts.push(f);
    }
    // A bare title is not worth overriding the page for — and it is exactly
    // what a mislabelled category page yields. Require at least one HARD fact
    // (price, availability, brand, rating, sku): those are the fields the
    // native fetcher cannot produce, because they live in metadata it drops
    // along with the rest of the page chrome.
    if facts.is_empty() {
        return None;
    }
    out.push_str(&format!("\n{}\n", facts.join(" · ")));
    // The page's own summary, when the body below turns out to be chrome.
    if let Some(d) =
        s(&ld, "description").or_else(|| crate::web::meta_content(html, "og:description"))
    {
        out.push_str(&format!("\n{d}\n"));
    }
    Some(out)
}

/// First object in any `application/ld+json` block whose `@type` is one of
/// `types`, searched depth-first so a `@graph` wrapper or an array payload is
/// handled. `@type` may itself be an array — schema.org allows both.
///
/// Matching is EXACT per type, never `contains`: `contains("Product")` also
/// matched `ProductGroup`/`ProductModel`, the schema.org types for the very
/// category listings the caller is trying to reject.
pub(super) fn json_ld_object(html: &str, types: &[&str]) -> Option<serde_json::Value> {
    for block in
        re(r#"(?is)<script\b[^>]*type\s*=\s*["']application/ld\+json["'][^>]*>(.*?)</script>"#)
            .captures_iter(html)
    {
        let Some(raw) = block.get(1) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.as_str().trim()) else {
            continue;
        };
        let mut stack = vec![v];
        while let Some(node) = stack.pop() {
            match node {
                serde_json::Value::Array(a) => stack.extend(a),
                serde_json::Value::Object(ref o) => {
                    let hit = o
                        .get("@type")
                        .map(|t| match t {
                            serde_json::Value::String(s) => types.contains(&s.as_str()),
                            serde_json::Value::Array(a) => a
                                .iter()
                                .any(|x| x.as_str().is_some_and(|s| types.contains(&s))),
                            _ => false,
                        })
                        .unwrap_or(false);
                    if hit {
                        return Some(node);
                    }
                    stack.extend(o.values().cloned());
                }
                _ => {}
            }
        }
    }
    None
}

/// First `@type: Product` object in any `application/ld+json` block.
pub(super) fn product_from_json_ld(html: &str) -> Option<serde_json::Value> {
    json_ld_object(html, &["Product"])
}

pub(super) fn crates_io_plan(path: &str) -> Option<Plan> {
    let name = path
        .strip_prefix("/crates/")?
        .split(['/', '?', '#'])
        .next()?;
    if name.is_empty() {
        return None;
    }
    Some(Plan::get(
        format!("https://crates.io/api/v1/crates/{name}"),
        Kind::CratesApi,
        "crates.io API",
    ))
}

pub(super) fn hf_dataset_plan(path: &str) -> Option<Plan> {
    let c = re(r"^/datasets/([^/]+)/([^/]+?)/?$").captures(path)?;
    let (owner, name) = (c.get(1)?.as_str(), c.get(2)?.as_str());
    Some(
        Plan::get(
            format!("https://huggingface.co/api/datasets/{owner}/{name}"),
            Kind::HuggingFaceRepo,
            "huggingface.co/api",
        )
        .with_aux(format!(
            "https://huggingface.co/datasets/{owner}/{name}/raw/main/README.md"
        )),
    )
}

pub(super) fn hf_org_plan(path: &str) -> Option<Plan> {
    let name = re(r"^/([^/]+?)/?$").captures(path)?.get(1)?.as_str();
    Some(
        Plan::get(
            format!("https://huggingface.co/api/models/{name}"),
            Kind::HuggingFaceRepo,
            "huggingface.co/api",
        )
        .with_aux(format!("https://huggingface.co/{name}/raw/main/README.md")),
    )
}

pub(super) fn hf_repo_plan(path: &str) -> Option<Plan> {
    let c = re(r"^/([^/]+)/([^/]+?)/?$").captures(path)?;
    let (owner, name) = (c.get(1)?.as_str(), c.get(2)?.as_str());
    Some(
        Plan::get(
            format!("https://huggingface.co/api/models/{owner}/{name}"),
            Kind::HuggingFaceRepo,
            "huggingface.co/api",
        )
        .with_aux(format!(
            "https://huggingface.co/{owner}/{name}/raw/main/README.md"
        )),
    )
}
