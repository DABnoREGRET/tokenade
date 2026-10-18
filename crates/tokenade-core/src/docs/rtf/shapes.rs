//! **The text of an RTF text box.**
//!
//! A text box is written `{\shp{\*\shpinst …{\shptxt TEXT}}{\shprslt …}}`:
//! the text lives under `\shpinst`, a STARRED destination, which the reader skips
//! entirely. `SIDEBAR boxed note` disappeared from the `.rtf` when the `.docx` and
//! the `.odt` of the same document served it (2026-10-04). Each shape that carries
//! a `\shptxt` is replaced by that text, as separate paragraphs; its
//! `\shprslt` (the fallback drawing for old readers) goes with it,
//! otherwise a reader that understands it would serve the text twice. A shape
//! WITHOUT text (floating image) stays intact: its image mark depends on it.

use super::rtf_group_end;

pub(super) fn with_text_box_text(src: &str) -> String {
    if !src.contains("\\shptxt") {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    // `{\shp` also catches isolated `{\shprslt`, `{\shptxt`: without `\shptxt`
    // inside, a group is copied as is.
    while let Some(at) = rest.find("{\\shp") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = rtf_group_end(tail) else {
            break;
        };
        let shape = &tail[..end];
        match shape
            .find("{\\shptxt")
            .and_then(|t| Some((t, rtf_group_end(&shape[t..])?)))
        {
            Some((t, len)) => {
                let inner = &shape[t + "{\\shptxt".len()..t + len - 1];
                out.push_str("{\\par ");
                out.push_str(inner);
                out.push_str("\\par}");
            }
            None => out.push_str(shape),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_box_reads_its_text_once_and_a_textless_shape_stays() {
        let rtf = r"{\rtf1 {\shp{\*\shpinst\shpleft1{\sp{\sn shapeType}{\sv 202}}{\shptxt\pard {SIDEBAR note}\par \pard}}{\shprslt {\*\do\dptxbx{\dptxbxtext SIDEBAR note}}}}First.\par {\shp{\*\shpinst{\sp{\sn pib}{\sv x}}}{\shprslt keep}}{\*\shppict{\pict\pngblip 89}}}";
        let out = with_text_box_text(rtf);
        assert_eq!(out.matches("SIDEBAR note").count(), 1, "{out}");
        assert!(
            out.contains(r"{\par \pard {SIDEBAR note}\par \pard\par}First."),
            "{out}"
        );
        assert!(
            out.contains(r"{\shp{\*\shpinst{\sp{\sn pib}{\sv x}}}{\shprslt keep}}{\*\shppict"),
            "{out}"
        );
        let text = crate::docs::extract_rtf(out.as_bytes());
        assert!(text.starts_with("SIDEBAR note\n\nFirst."), "{text:?}");
    }
}
