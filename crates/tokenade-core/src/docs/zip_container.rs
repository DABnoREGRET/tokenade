//! **The ZIP container: open, bound, recognise.**
//!
//! OOXML, ODF and EPUB are all zip archives, and three questions come back for
//! each: WHAT (`format_from_zip`, which looks at what is INSIDE, because entry
//! order is not specified and real producers differ), HOW MUCH (the expansion
//! caps, calibrated on 224 real documents), and HOW (opening the archive,
//! reading an XML entry).
//!
//! Split out of `docs.rs` on 2026-09-13: the plumbing of a container format is
//! not the format dispatcher, and `docs/pdf.rs`, `docs/epub.rs`,
//! `docs/spreadsheet.rs` and `docs/tables.rs` already had their own file.

use super::Format;
use std::io::Cursor;
use zip::ZipArchive;

/// Identify an OOXML/ODF/EPUB container by what is INSIDE the zip.
///
/// Signature sniffers (ours included, via `infer`) key off the FIRST zip entry,
/// but zip entry order is not specified and real producers differ: LibreOffice
/// writes `_rels/.rels` first in a .docx, so the file sniffs as a plain "zip",
/// no extractor runs, and `tokenade web` fell through to the HTML path and
/// printed the raw compressed bytes at the agent (measured on a LibreOffice
/// .docx — a screenful of binary, billed as tokens). Its .xlsx only worked by
/// luck: LibreOffice happens to put `xl/_rels/…` first, which the sniffer
/// accepts. So: look for the parts the SPEC requires, wherever they sit.
pub(crate) fn format_from_zip(bytes: &[u8]) -> Option<Format> {
    format_from_zip_seek(Cursor::new(bytes))
}

/// Identify a zip-container document from anything seekable.
///
/// Generic over the reader so a caller with a FILE does not have to slurp it
/// first: a zip's central directory lives at the END, so identification needs
/// seeking, not the whole archive in memory. Making this take `&[u8]` forced
/// every caller to `fs::read` the entire file — a 500 MB archive read into RAM
/// to learn three bytes' worth of "it's a docx". `ZipArchive` only ever needs
/// `Read + Seek`, and `File` provides both.
pub(crate) fn format_from_zip_seek<R: std::io::Read + std::io::Seek>(reader: R) -> Option<Format> {
    let mut zip = open_zip(reader)?;
    // ODF/EPUB declare themselves in a `mimetype` entry — authoritative.
    if let Ok(f) = zip.by_name("mimetype") {
        use std::io::Read;
        let mut s = String::new();
        // A `mimetype` entry is ~40 bytes by specification. Reading it whole
        // to learn the format is how a bomb wins before the format is even
        // known, so this one is capped hard rather than generously.
        const MAX_MIMETYPE_BYTES: u64 = 1024;
        if f.take(MAX_MIMETYPE_BYTES).read_to_string(&mut s).is_ok() {
            let s = s.trim();
            if s.contains("opendocument.text") {
                return Some(Format::Odt);
            }
            if s.contains("opendocument.spreadsheet") {
                return Some(Format::Ods);
            }
            if s.contains("opendocument.presentation") {
                return Some(Format::Odp);
            }
            if s.contains("epub+zip") {
                return Some(Format::Epub);
            }
        }
    }
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();
    let has = |p: &str| names.iter().any(|n| n == p);
    let any_under = |p: &str| names.iter().any(|n| n.starts_with(p));
    if has("word/document.xml") {
        return Some(Format::Docx);
    }
    if has("xl/workbook.xml") || any_under("xl/worksheets/") {
        return Some(Format::Xlsx);
    }
    if has("ppt/presentation.xml") || any_under("ppt/slides/") {
        return Some(Format::Pptx);
    }
    // EPUB without a mimetype entry (out of spec, but seen in the wild).
    if has("META-INF/container.xml") {
        return Some(Format::Epub);
    }
    None
}

// ---- decompression budget --------------------------------------------------

/// Most bytes one zip entry may expand to, and most an archive may expand to
/// in total.
///
/// A `.docx`/`.xlsx`/`.epub`/`.pptx` is a ZIP, and a ZIP's expansion ratio is
/// unbounded. Measured 2026-08-28: a **400 KB** file whose `word/document.xml`
/// expands to 300 MB drove `tokenade read` to **3.5 GB resident and 627
/// seconds** before it was killed — a small file that hangs the agent's Read
/// and takes the machine with it. Neither read site had a bound: both did a
/// plain `read_to_string` on the entry.
///
/// The caps are calibrated, not guessed. Over **224 real documents** on this
/// machine (EPUBs, spreadsheets, Word test corpora): the largest SINGLE entry
/// is 9.78 MB, the 95th percentile 0.37 MB, the median 0.030 MB; the largest
/// TOTAL expansion is 16.05 MB. 64 MB per entry is six times the largest real
/// part, and 256 MB per archive sixteen times the largest real document.
///
/// The total is read from the central directory, so an over-budget archive is
/// refused BEFORE a byte is decompressed. The per-entry cap is enforced with
/// `take` as well, because the central directory is written by whoever made
/// the file and can lie.
const MAX_ZIP_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ZIP_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// The archive-wide budget, overridable like every other ceiling in this
/// product (`TOKENADE_MAX_INPUT_BYTES`, `TOKENADE_RAW_BUDGET`,
/// `TOKENADE_DISCLOSE_THRESHOLD`). One reader, so the DECLARED check and the
/// running total below cannot be given different ceilings.
fn zip_total_budget() -> u64 {
    std::env::var("TOKENADE_MAX_ZIP_TOTAL_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(MAX_ZIP_TOTAL_BYTES)
}

thread_local! {
    /// What is LEFT of [`MAX_ZIP_TOTAL_BYTES`] for the archive this thread is
    /// extracting.
    ///
    /// The declared total above is read from the central directory, and the
    /// comment there already says that directory "can lie" — it answered the
    /// lie per ENTRY and never across them. Measured 2026-09-06: a 493 765-byte
    /// `.pptx` declaring 12 KB of content, whose twelve slides each expand to
    /// 40 MB, came back with `tokens=62914743` after **2 883 MB resident and
    /// 41 seconds**. Every entry passed the per-entry cap honestly; there was
    /// no running total for them to fail.
    ///
    /// Armed by [`open_zip`] — the same single declarer that refuses an
    /// over-declared archive — so the two halves of one budget cannot drift
    /// apart. Thread-local because a batch extracts in parallel and each
    /// document is entitled to its own budget, not a share of one.
    static ZIP_BUDGET_LEFT: std::cell::Cell<u64> = const { std::cell::Cell::new(MAX_ZIP_TOTAL_BYTES) };
}

/// Open a document container, refusing one that declares more expansion than
/// [`MAX_ZIP_TOTAL_BYTES`]. THE single declarer — both the extractor and the
/// format sniffer go through it, so a bound added here cannot be missing there.
pub(crate) fn open_zip<R: std::io::Read + std::io::Seek>(mut reader: R) -> Option<ZipArchive<R>> {
    let mut zip = ZipArchive::new(&mut reader).ok()?;
    let mut declared: u64 = 0;
    for i in 0..zip.len() {
        let Ok(f) = zip.by_index_raw(i) else { continue };
        declared = declared.saturating_add(f.size());
        if declared > zip_total_budget() {
            return None;
        }
    }
    // The declaration passed. Arm the budget that catches it having lied.
    ZIP_BUDGET_LEFT.with(|b| b.set(zip_total_budget()));
    drop(zip);
    ZipArchive::new(reader).ok()
}

// ---- per-format extractors ------------------------------------------------

/// Read a zip entry as raw text (no XML→text pass). The XLSX extractor needs
/// the MARKUP, because the cell boundaries live in the tags.
pub(crate) fn raw_xml_entry<R: std::io::Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    path: &str,
) -> String {
    let Ok(file) = zip.by_name(path) else {
        return String::new();
    };
    // The declared size is a claim by whoever wrote the file; `take` is the
    // guarantee. Both are here on purpose — the first skips the work, the
    // second bounds it when the first was lied to.
    if file.size() > MAX_ZIP_ENTRY_BYTES {
        return String::new();
    }
    // ...and what is left of the ARCHIVE's budget bounds it across entries. A
    // per-entry cap alone multiplies by the entry count, which is the caller's
    // to choose, not ours.
    let left = ZIP_BUDGET_LEFT.with(|b| b.get());
    let mut xml = String::new();
    use std::io::Read;
    if file
        .take(MAX_ZIP_ENTRY_BYTES.min(left))
        .read_to_string(&mut xml)
        .is_err()
    {
        return String::new();
    }
    ZIP_BUDGET_LEFT.with(|b| b.set(left.saturating_sub(xml.len() as u64)));
    xml
}
