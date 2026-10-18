//! What is left once the markup is gone: tags dropped, entities decoded.
//!
//! The last lexical pass of the converter, and the only place that knows the
//! HTML entity table.

use super::*;

/// Remove every remaining HTML tag (`<…>`), tolerating `>` inside quoted
/// attribute values (see [`TAG_ATTRS`]).
pub(super) fn strip_all_tags(s: &str) -> String {
    re(&format!(r"(?s)<[a-zA-Z/!?]{TAG_ATTRS}>"))
        .replace_all(s, "")
        .into_owned()
}

/// Decode the HTML entities a content extractor cares about.
///
/// The set used to stop at punctuation, on the reasoning that anything else was
/// "rare enough" to leave raw. That reasoning was wrong in a way that only
/// shows on non-English pages: `&auml;` is not exotic, it is every German page,
/// and `&eacute;` every French one. A German search result reached the agent as
/// `Welche Dezibelwerte sind f&uuml;r ein Klimager&auml;t im Schlafzimmer` —
/// readable, but wrong, and wrong in the delivered text.
///
/// So the LETTER entities are complete (all of Latin-1, both cases) while the
/// symbol set stays curated. Still no 1 MB table: anything past this is genuinely
/// rare, and a raw `&xyz;` beats a dependency.
pub fn decode_entities(s: &str) -> String {
    let pairs: &[(&str, &str)] = &[
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&apos;", "'"),
        ("&#39;", "'"),
        ("&mdash;", "—"),
        ("&ndash;", "–"),
        ("&hellip;", "…"),
        ("&laquo;", "«"),
        ("&raquo;", "»"),
        ("&copy;", "©"),
        // Doc-generator headerlink artifact (`<a>&para;</a>` after every
        // heading) — decoding to empty drops the noise.
        ("&para;", ""),
        ("&sect;", "§"),
        ("&middot;", "·"),
        ("&bull;", "•"),
        ("&reg;", "®"),
        ("&trade;", "™"),
        ("&aacute;", "á"),
        ("&agrave;", "à"),
        ("&acirc;", "â"),
        ("&auml;", "ä"),
        ("&atilde;", "ã"),
        ("&aring;", "å"),
        ("&eacute;", "é"),
        ("&egrave;", "è"),
        ("&ecirc;", "ê"),
        ("&euml;", "ë"),
        ("&iacute;", "í"),
        ("&igrave;", "ì"),
        ("&icirc;", "î"),
        ("&iuml;", "ï"),
        ("&itilde;", "ĩ"),
        ("&oacute;", "ó"),
        ("&ograve;", "ò"),
        ("&ocirc;", "ô"),
        ("&ouml;", "ö"),
        ("&otilde;", "õ"),
        ("&uacute;", "ú"),
        ("&ugrave;", "ù"),
        ("&ucirc;", "û"),
        ("&uuml;", "ü"),
        ("&utilde;", "ũ"),
        ("&uring;", "ů"),
        ("&yacute;", "ý"),
        ("&ycirc;", "ŷ"),
        ("&yuml;", "ÿ"),
        ("&Aacute;", "Á"),
        ("&Agrave;", "À"),
        ("&Acirc;", "Â"),
        ("&Auml;", "Ä"),
        ("&Atilde;", "Ã"),
        ("&Aring;", "Å"),
        ("&Eacute;", "É"),
        ("&Egrave;", "È"),
        ("&Ecirc;", "Ê"),
        ("&Euml;", "Ë"),
        ("&Iacute;", "Í"),
        ("&Igrave;", "Ì"),
        ("&Icirc;", "Î"),
        ("&Iuml;", "Ï"),
        ("&Itilde;", "Ĩ"),
        ("&Oacute;", "Ó"),
        ("&Ograve;", "Ò"),
        ("&Ocirc;", "Ô"),
        ("&Ouml;", "Ö"),
        ("&Otilde;", "Õ"),
        ("&Uacute;", "Ú"),
        ("&Ugrave;", "Ù"),
        ("&Ucirc;", "Û"),
        ("&Uuml;", "Ü"),
        ("&Utilde;", "Ũ"),
        ("&Uring;", "Ů"),
        ("&Yacute;", "Ý"),
        ("&Ycirc;", "Ŷ"),
        ("&Yuml;", "Ÿ"),
        ("&ccedil;", "ç"),
        ("&Ccedil;", "Ç"),
        ("&ntilde;", "ñ"),
        ("&Ntilde;", "Ñ"),
        ("&szlig;", "ß"),
        ("&aelig;", "æ"),
        ("&AElig;", "Æ"),
        ("&oslash;", "ø"),
        ("&Oslash;", "Ø"),
        ("&eth;", "ð"),
        ("&ETH;", "Ð"),
        ("&thorn;", "þ"),
        ("&THORN;", "Þ"),
        ("&iexcl;", "¡"),
        ("&iquest;", "¿"),
        ("&deg;", "°"),
        ("&plusmn;", "±"),
        ("&frac12;", "½"),
        ("&frac14;", "¼"),
        ("&frac34;", "¾"),
        ("&times;", "×"),
        ("&divide;", "÷"),
        ("&euro;", "€"),
        ("&pound;", "£"),
        ("&yen;", "¥"),
        ("&cent;", "¢"),
        ("&dagger;", "†"),
        ("&permil;", "‰"),
        ("&lsaquo;", "‹"),
        ("&rsaquo;", "›"),
        ("&sbquo;", "‚"),
        ("&bdquo;", "„"),
        ("&circ;", "ˆ"),
        ("&tilde;", "˜"),
        ("&minus;", "−"),
    ];
    let mut out = s.to_string();
    for (from, to) in pairs {
        out = out.replace(from, to);
    }
    // Decode numeric entities (decimal): &#NNN;
    let r = re(r"&#(\d{1,7});");
    out = r
        .replace_all(&out, |c: &regex::Captures| {
            c.get(1)
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .and_then(char::from_u32)
                .map(|ch| ch.to_string())
                .unwrap_or_default()
        })
        .into_owned();
    // Hex numeric entities: &#xNNN;
    let rh = re(r"&#[xX]([0-9a-fA-F]{1,6});");
    out = rh
        .replace_all(&out, |c: &regex::Captures| {
            u32::from_str_radix(c.get(1).unwrap().as_str(), 16)
                .ok()
                .and_then(char::from_u32)
                .map(|ch| ch.to_string())
                .unwrap_or_default()
        })
        .into_owned();
    out
}
