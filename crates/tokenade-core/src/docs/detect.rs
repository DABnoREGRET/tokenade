//! **What is this file?**
//!
//! The list of formats tokenade extracts, the table of known
//! extensions, and the three ways to answer the question: by the announced
//! extension, by the magic bytes, or — for a zip container — by what
//! is inside it.
//!
//! Moved out of `docs.rs` on 2026-09-13. Recognising a format and EXTRACTING it are
//! two jobs, and keeping them in the same file is what made
//! the document dispatcher a 2 715-line file.

use super::pdf::pdf_magic;
use super::zip_container::{format_from_zip, format_from_zip_seek};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Docx,
    Xlsx,
    Pptx,
    Odt,
    Ods,
    Odp,
    Epub,
    Pdf,
    /// OpenDocument Drawing — same `content.xml` as the other ODF formats.
    Odg,
    /// Flat ODF (`.fodt`/`.fods`/`.fodp`/`.fodg`): the whole document as ONE
    /// uncompressed XML file instead of a zip container.
    FlatOdf,
    /// Rich Text Format — control words in plain text, no container.
    Rtf,
    /// FictionBook 2 — a single XML ebook.
    Fb2,
    /// Legacy Excel (BIFF, pre-2007) and the binary .xlsb. OLE2/CFB containers,
    /// not zip — parsed by `calamine`, which is pure Rust (no libreoffice
    /// shell-out, so it works on every platform we ship).
    Xls,
    /// Legacy Word (97–2003): the same OLE2/CFB container as `Xls`, told apart
    /// by its `WordDocument` stream.
    Doc,
    /// Legacy PowerPoint (97–2003), OLE2 again, by its `PowerPoint Document`
    /// stream.
    Ppt,
    /// An Outlook message (OLE2, MAPI properties as streams).
    Msg,
}

/// Every extension we route, as data. A match arm can only be *asked* about an
/// extension you already suspect; this can be enumerated, which is what lets
/// the rules we scaffold be checked against what we really handle — in BOTH
/// directions. A format that ships but is never advertised is a format users
/// do not have.
pub const KNOWN_EXTENSIONS: &[(&str, Format)] = &[
    // OOXML. Macro-enabled (`m`) and template (`t`) variants are the SAME
    // zip+XML container with a different extension — enterprise trees are full
    // of .docm/.xlsm/.dotx, and rejecting them for a letter was a gap, not a
    // design.
    ("docx", Format::Docx),
    ("docm", Format::Docx),
    ("dotx", Format::Docx),
    ("dotm", Format::Docx),
    ("xlsx", Format::Xlsx),
    ("xlsm", Format::Xlsx),
    ("xltx", Format::Xlsx),
    ("xltm", Format::Xlsx),
    ("pptx", Format::Pptx),
    ("pptm", Format::Pptx),
    ("potx", Format::Pptx),
    ("potm", Format::Pptx),
    ("ppsx", Format::Pptx),
    ("ppsm", Format::Pptx),
    // OpenDocument, incl. templates (`ot*`) and Draw. Drawings carry the same
    // `content.xml` as text documents, so the text in a diagram is extractable
    // with the existing path.
    ("odt", Format::Odt),
    ("ott", Format::Odt),
    ("ods", Format::Ods),
    ("ots", Format::Ods),
    ("odp", Format::Odp),
    ("otp", Format::Odp),
    ("odg", Format::Odg),
    ("otg", Format::Odg),
    // Flat ODF: one uncompressed XML file, no zip container.
    ("fodt", Format::FlatOdf),
    ("fods", Format::FlatOdf),
    ("fodp", Format::FlatOdf),
    ("fodg", Format::FlatOdf),
    ("epub", Format::Epub),
    ("pdf", Format::Pdf),
    // `.xlsm` is OOXML (zip), NOT legacy BIFF — routing it here gave the same
    // workbook a different renderer depending on its extension.
    ("xls", Format::Xls),
    ("xlsb", Format::Xls),
    // Word 97–2003 and its template: OLE2, not zip.
    ("doc", Format::Doc),
    ("dot", Format::Doc),
    ("ppt", Format::Ppt),
    ("pps", Format::Ppt),
    ("pot", Format::Ppt),
    ("msg", Format::Msg),
    // Rich Text: not a container at all — control words in plain ASCII.
    ("rtf", Format::Rtf),
    // FictionBook: a single XML ebook, common in non-English corpora.
    ("fb2", Format::Fb2),
];

impl Format {
    /// Detect from a filename extension (case-insensitive). Returns `None`
    /// for unrecognised extensions.
    pub fn from_extension(ext: &str) -> Option<Format> {
        let ext = ext.to_ascii_lowercase();
        KNOWN_EXTENSIONS
            .iter()
            .find(|(name, _)| *name == ext)
            .map(|(_, fmt)| *fmt)
    }

    /// Detect from a filename (extension portion). Returns `None` if there is
    /// no recognised extension.
    pub fn from_filename(name: &str) -> Option<Format> {
        let ext = name.rsplit('.').next().unwrap_or("");
        Self::from_extension(ext)
    }
}

/// Identify a document from its NAME and its BYTES — the single detection entry
/// point every channel must use.
///
/// `hint` is whatever the channel knows: a file path (`Read`), a URL or
/// content-type-derived kind (`WebFetch`), a CLI argument (`tokenade web`). The
/// name is trusted first because it is cheap and usually right; the bytes settle
/// it when the name says nothing.
///
/// This exists because the three channels each rolled their own detection, and
/// the content-based sniffing added for the LibreOffice DOCX bug (zip entries
/// are in NO specified order, so LibreOffice's `_rels/.rels`-first archives were
/// mis-sniffed and the raw bytes were vomited as text) only ever landed on ONE
/// of them. A fix that reaches a third of the product is a fix that will be
/// re-reported by users on the other two.
pub fn detect_format(hint: Option<&str>, bytes: &[u8]) -> Option<Format> {
    if let Some(fmt) = hint.and_then(Format::from_filename) {
        // An OLE2 name proves nothing: content sniffers label EVERY such
        // container `doc`, so `x.doc` arrived for a `.xls` and the workbook was
        // handed to the Word reader. Inside that container, the streams decide.
        if matches!(fmt, Format::Doc | Format::Xls | Format::Ppt | Format::Msg)
            && bytes.starts_with(&CFB_MAGIC)
        {
            return super::word97::cfb_format(bytes);
        }
        return Some(fmt);
    }
    if pdf_magic(bytes) {
        return Some(Format::Pdf);
    }
    // Content sniffing for the NON-zip document formats. A file arriving over
    // the wire (a WebFetch that redirects to a download, `curl … | tokenade
    // web -`) often has no usable name, and without these a .rtf/.xls/.fb2/
    // flat-ODF fell through to the text path — the same "served as raw bytes"
    // failure the zip sniffing was added to fix, just for the other half of the
    // formats.
    if let Some(fmt) = format_from_magic(bytes) {
        return Some(fmt);
    }
    format_from_zip(bytes)
}

/// The OLE2 / Compound File Binary signature.
const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Identify the non-zip, non-PDF document formats from their leading bytes.
fn format_from_magic(bytes: &[u8]) -> Option<Format> {
    // RTF is literally `{\rtf` in ASCII.
    // `{` `\` `r` `t` `f` as explicit bytes: escaping this literal through
    // tooling already corrupted it once (a raw control char landed inside
    // the string and the sniff silently never matched).
    if bytes.starts_with(&[0x7B, 0x5C, 0x72, 0x74, 0x66]) {
        return Some(Format::Rtf);
    }
    // OLE2/CFB is a CONTAINER, shared by legacy Word, Excel and PowerPoint,
    // Outlook's `.msg` and password-protected OOXML. Answering `Xls` for every
    // one of them made a `.ppt` or a `.msg` "a spreadsheet that yielded no
    // text", and the binary notice then told the agent « tokenade reads ppt
    // files; … most likely truncated, empty, or encrypted » of a healthy file.
    // A format is claimed only from the stream that proves it.
    if bytes.starts_with(&CFB_MAGIC) {
        return super::word97::cfb_format(bytes);
    }
    // XML-rooted formats: look only at a bounded prefix, and only at the ROOT
    // element, so a document merely MENTIONING these names is not misread.
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    if text.contains("<FictionBook") {
        return Some(Format::Fb2);
    }
    // Flat ODF declares the ODF office namespace and carries its body inline.
    if text.contains("office:document") && text.contains("opendocument") {
        return Some(Format::FlatOdf);
    }
    None
}

/// [`detect_format`] for a file on disk, WITHOUT reading it into memory.
///
/// Identification needs at most a 1 KiB header (PDF magic) plus the zip central
/// directory, which `ZipArchive` seeks to itself. Reading the whole file to
/// answer "what is this" is pure waste on the Read hook's hot path, where the
/// file may be huge and, most of the time, the NAME already settles it.
pub fn detect_format_at(path: &std::path::Path) -> Option<Format> {
    let named = path.to_str().and_then(Format::from_filename);
    if let Some(fmt) =
        named.filter(|f| !matches!(f, Format::Doc | Format::Xls | Format::Ppt | Format::Msg))
    {
        return Some(fmt);
    }
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = Vec::with_capacity(4096);
    std::io::Read::read_to_end(&mut std::io::Read::take(&mut f, 4096), &mut head).ok()?;
    // An OLE2 container is told apart by its streams, which needs the whole
    // file — the same rule `detect_format` applies to bytes in memory.
    if head.starts_with(&CFB_MAGIC) {
        return super::word97::cfb_format(&std::fs::read(path).ok()?);
    }
    if named.is_some() {
        return named;
    }
    if pdf_magic(&head) {
        return Some(Format::Pdf);
    }
    // The other non-zip formats, sniffed as `detect_format` sniffs them: a
    // `.rtf` or a flat ODF saved without its extension was read as text here
    // and as a document over the wire.
    if let Some(fmt) = format_from_magic(&head) {
        return Some(fmt);
    }
    std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(0)).ok()?;
    format_from_zip_seek(f)
}
