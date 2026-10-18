//! **Rich Text Format → text, by hand.**
//!
//! RTF needs no library: it is ASCII with control words
//! `\control`, `{}` groups and hexadecimal escapes
//! `\'xx`. Moved out of `docs.rs` on 2026-09-13 — 404 lines of a complete
//! parser in the middle of a format dispatcher, and the only module of the
//! family that did not have its own file while `pdf`, `epub`, `spreadsheet`
//! and `tables` have theirs.
//!
//! It names only `markdown`.

mod shapes;
use std::fmt::Write;

/// Consume ONE `\uN` fallback unit — the representation that follows a
/// Unicode escape for readers that cannot handle it.
///
/// The old rule skipped the fallback only when it did not start with a
/// backslash, and LibreOffice writes it as `\'hh`. So every accented
/// character in an RTF came out TWICE: `\u233\'e9lusif` was read as
/// "éélusif", `\u171\'ab` as "««". Word writes a plain `?` and was fine,
/// which is how a format that IS supported could be corrupting every document
/// produced by the other half of the world.
///
/// One unit is: `\'hh`, an escaped `\\` / `\{` / `\}`, or a single plain
/// character. A real control word (`\par`) is NOT a fallback, and neither is
/// a group delimiter; both return false so the caller stops skipping.
fn skip_rtf_fallback_unit(chars: &mut std::iter::Peekable<std::str::Chars>) -> bool {
    match chars.peek().copied() {
        Some('\\') => {
            let mut probe = chars.clone();
            probe.next();
            match probe.peek().copied() {
                Some('\'') => {
                    chars.next();
                    chars.next();
                    chars.next();
                    chars.next();
                    true
                }
                Some('\\') | Some('{') | Some('}') => {
                    chars.next();
                    chars.next();
                    true
                }
                _ => false,
            }
        }
        Some('{') | Some('}') | None => false,
        Some(_) => {
            chars.next();
            true
        }
    }
}

/// **An RTF image's caption lives IN the group that is thrown away.**
///
/// `{\pict …}` carries the image's bytes and goes whole, rightly. But
/// LibreOffice also stores there the replacement text the author typed, under
/// `{\sp{\sn wzDescription}{\sv …}}` — so the caption went away with the bytes.
/// It is the image-caption fix in a third format, and the mark rendered is the same as
/// on a web page and in an Office document.
///
/// **A caption carrying an RTF escape degrades to the BARE mark.** A
/// `\'e9` or a `\uNNNN` rendered as is would fabricate characters the
/// document does not contain, and decoding it here would require replaying
/// the parser on a fragment. A mark without a caption says less; it says
/// nothing false.
fn rtf_pictures_as_marks(src: &str) -> String {
    if !src.contains("{\\pict") {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find("{\\pict") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = rtf_group_end(tail) else {
            // Group never closed (truncated file). The previous reader
            // muted everything that followed, and what follows is the image's
            // base64: we keep that choice, and SAY it through the mark instead
            // of keeping quiet.
            out.push(' ');
            out.push_str(&crate::markdown::image_marker(None));
            return out;
        };
        let (group, after) = tail.split_at(end);
        // **`wzName` is NOT read, and that is a decision, not an oversight.**
        //
        // The RTF does carry two texts — `wzDescription`, the long
        // description, and `wzName` — and the second is what LibreOffice writes there
        // when the author gave their figure a title: the corpus's `.rtf` carries
        // `NEEDLE-IMG-TITLE` there. But `wzName` is ALSO where
        // the name the software invents by itself lands when nobody
        // typed anything (`Image1`, `Picture 3`, `Bild 1`), and nothing in the file
        // distinguishes the two cases. OOXML separates them (`title=` typed,
        // `name=` made up), RTF conflates them.
        //
        // Serving `wzName` would therefore make `[image: Image1 — …]` appear on
        // every untitled figure: a word the author never wrote,
        // at the price of a title gained on those that have one. Measured:
        // the spellings ratchet stays at `needle-img-title servi par
        // notes.odt notes.docx ABSENT de notes.rtf` — a NAMED divergence
        // is better than a silent fabrication. A list of auto-generated names
        // is not a discriminant: they are translated.
        let alt = super::re(r"(?s)\{\\sn wzDescription\}\{\\sv ([^}]*)\}")
            .captures(group)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .filter(|a| !a.contains('\\'));
        out.push(' ');
        out.push_str(&crate::markdown::image_marker(alt));
        out.push(' ');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The index just AFTER the brace that closes the group opened at the first
/// character. `\{` and `\}` are LITERAL braces and do not count.
pub(super) fn rtf_group_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1, // the escape consumes the next character
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}
/// Style number → heading level, from the `{\stylesheet}` group.
///
/// A LibreOffice or Word RTF marks a heading by its STYLE (`\s1`, declared
/// `{\s1 … heading 1;}`) and often writes no `\outlinelevel` at all: the
/// three headings of a `.docx` converted to `.rtf` read as plain lines while
/// the `.docx`, `.odt` and `.pdf` of the same report showed `#`, `##`, `###`
/// (2026-10-03). The style NAME decides, through the same multilingual table
/// the other readers use.
fn heading_styles(src: &str) -> std::collections::HashMap<u32, usize> {
    let mut map = std::collections::HashMap::new();
    let Some(at) = src.find("{\\stylesheet") else {
        return map;
    };
    let Some(len) = rtf_group_end(&src[at..]) else {
        return map;
    };
    let sheet = &src[at + 1..at + len - 1];
    let word = tk_base::regexes::re(r"\\[a-zA-Z]+-?\d* ?|[{}]");
    for entry in sheet.split(';') {
        let Some(num) = entry.find("\\s").and_then(|i| {
            let d: String = entry[i + 2..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            d.parse::<u32>().ok()
        }) else {
            continue;
        };
        let name = word.replace_all(entry, "");
        if let Some(level) = super::heading_level_from_style(name.trim()) {
            map.insert(num, level);
        }
    }
    map
}

// Rich Text Format → plain text, hand-rolled because RTF needs no library:
// it is ASCII with `\\control` words, `{}` groups and `\\'xx` hex escapes.
//
// The parts that matter for correctness:
//  * Groups starting `{\\*\\...}` are DESTINATIONS we must skip whole
//    (`fonttbl`, `colortbl`, `stylesheet`, `info`, embedded objects) — their
//    contents are metadata, and emitting them buries the document's text in
//    font names.
//  * `\\par`/`\\line` are breaks; `\\tab` is a tab; `\\'xx` is a byte in the
//    current codepage (we map the Latin-1 range, which covers Western RTF).
//  * A control word ends at a non-alphanumeric, and ONE following space is
//    part of the delimiter, not the text.

/// Marks the end of a table cell in the raw extraction; [`rows_as_tables`]
/// turns each run of marked lines into a markdown table.
const CELL_END: char = '\u{1F}';

/// Ends a table row, followed by the right edges (`\cellxN`) of its cells:
/// the grid a merged cell is placed against ([`rows_as_tables`]).
const ROW_EDGES: char = '\u{1E}';

/// Consecutive lines carrying [`CELL_END`] are the rows of one table: rendered
/// by the same writer as every other format's tables.
fn rows_as_tables(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rows: Vec<Vec<String>> = Vec::new();
    let flush = |rows: &mut Vec<Vec<String>>, out: &mut String| {
        if !rows.is_empty() {
            out.push('\n');
            out.push_str(&crate::markdown::rows_to_markdown(rows));
            out.push('\n');
            rows.clear();
        }
    };
    // Each row's cell edges, kept until the table is complete: a merged cell
    // is placed against the grid of EVERY row's edges.
    let mut edges: Vec<Vec<i64>> = Vec::new();
    let flush = |rows: &mut Vec<Vec<String>>, edges: &mut Vec<Vec<i64>>, out: &mut String| {
        super::tables::spread_merged_cells(rows, edges);
        edges.clear();
        flush(rows, out);
    };
    for line in text.split('\n') {
        if line.contains(CELL_END) {
            let (line, row_edges) = line.split_once(ROW_EDGES).unwrap_or((line, ""));
            let mut cells: Vec<String> = line
                .split(CELL_END)
                .map(|c| c.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect();
            if cells.last().is_some_and(|c| c.is_empty()) {
                cells.pop();
            }
            rows.push(cells);
            edges.push(
                row_edges
                    .split(',')
                    .filter_map(|e| e.trim().parse().ok())
                    .collect(),
            );
        } else {
            flush(&mut rows, &mut edges, &mut out);
            // A `\row` with no cell still carries its edges: they are not text.
            out.push_str(line.split(ROW_EDGES).next().unwrap_or(line));
            out.push('\n');
        }
    }
    flush(&mut rows, &mut edges, &mut out);
    out
}

/// The printable label inside a `{\listtext …}` group (`1.`, `a)`), control
/// words, hex escapes and `\uN` glyphs left out; the iterator starts just
/// after the group's `{`.
fn listtext_label(mut it: std::iter::Peekable<std::str::Chars>) -> String {
    let mut label = String::new();
    let mut nest = 0i32;
    while let Some(c) = it.next() {
        match c {
            '{' => nest += 1,
            '}' if nest == 0 => break,
            '}' => nest -= 1,
            // A glyph written as `\'hh` or `\uN` (a bullet, in practice) is
            // kept as a non-alphanumeric placeholder: the group HAS a marker,
            // where a heading's outline group holds nothing but `\tab`.
            '\\' => match it.peek() {
                Some('\'') => {
                    it.next();
                    it.next();
                    it.next();
                    if !label.ends_with('•') {
                        label.push('•');
                    }
                }
                Some(&ch) if ch.is_ascii_alphabetic() => {
                    let mut word = String::new();
                    while let Some(ch) = it
                        .peek()
                        .copied()
                        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
                    {
                        word.push(ch);
                        it.next();
                    }
                    if word.len() > 1
                        && word.starts_with('u')
                        && word[1..].chars().all(|c| c.is_ascii_digit() || c == '-')
                        && !label.ends_with('•')
                    {
                        label.push('•');
                    }
                    if it.peek() == Some(&' ') {
                        it.next();
                    }
                }
                _ => {
                    it.next();
                }
            },
            _ => label.push(c),
        }
    }
    label.trim().to_string()
}

/// The URL of a `HYPERLINK "…"` field instruction, read from just after the
/// group's `{`. `HYPERLINK \\l "anchor"` (a jump inside the document) has none.
fn hyperlink_target(it: std::iter::Peekable<std::str::Chars>) -> Option<String> {
    let instr = field_instruction(it);
    super::fields::hyperlink_url(&instr[instr.find("HYPERLINK")?..])
}

/// A field's instruction, read just after the `{` of its `\*\fldinst` group
/// (the braces of a nested group do not count).
fn field_instruction(it: std::iter::Peekable<std::str::Chars>) -> String {
    let mut nest = 0i32;
    let mut instr = String::new();
    for c in it {
        match c {
            '{' => nest += 1,
            '}' if nest == 0 => break,
            '}' => nest -= 1,
            _ => instr.push(c),
        }
    }
    instr
        .split_once("fldinst")
        .map_or(instr.as_str(), |(_, rest)| rest)
        .trim()
        .to_string()
}

pub(crate) fn extract_rtf(bytes: &[u8]) -> String {
    let src = String::from_utf8_lossy(bytes);
    let src = rtf_pictures_as_marks(&shapes::with_text_box_text(
        &super::formulas::with_rtf_formulas(&src),
    ));
    let mut out = String::new();
    let mut chars = src.chars().peekable();
    // **A note is not a word of the sentence that calls it.**
    //
    // RTF writes the WHOLE note at the place of its call, like
    // OpenDocument: `…carries the argument.{\*\footnote \chftn
    // NOTE-BASSE…}`. The reader therefore rendered it in the middle of the body, separated from the
    // last word by the single tab the group contains — so a
    // sentence cut in two with nothing saying where it resumes, when the
    // `.docx` and the `.odt` of the same source store it at the end of the text. What
    // the text emits is routed here, and the two classes are separated by what
    // the document DECLARES: `\ftnalt` says « endnote ».
    let mut notes: [String; 2] = Default::default();
    let mut note_depth: i32 = 0;
    let mut note_kind: usize = 0;
    let mut note_count = [0usize; 2];
    // `\super`/`\sub` until `\nosupersub` or the group closes: (group depth,
    // marker, already decided). The marker goes before the first character
    // unless that is a letter — the web reader's rule (`raised_and_lowered`):
    // `1648{\super 1}` read `16481`, a year the document never printed.
    let mut raised: Option<(i32, char, bool)> = None;
    // Text that Word does not display — hidden (`\v`) or deleted under tracked
    // changes (`\deleted`) — up to `\v0`/`\deleted0`/`\plain` or the
    // end of the group that opened it: dropped as in `.docx`/`.odt`/`.doc`. The
    // line ends stay, so as not to weld two paragraphs.
    let mut unseen: Option<i32> = None;
    macro_rules! emit {
        ($x:expr) => {{
            let ch: char = $x;
            if unseen.is_some() && ch != '\n' {
            } else if note_depth > 0 {
                // A note's body starts with its own `\chftn` and a
                // space: under `[^1]: `, that whitespace is not text.
                if !(ch.is_whitespace() && notes[note_kind].ends_with(": ")) {
                    notes[note_kind].push(ch)
                }
            } else {
                out.push(ch)
            }
        }};
    }
    // Depth of nested groups we are skipping (0 = emitting).
    // `\ucN` — how many fallback units follow each `\uN`. 1 is the spec's
    // default and what LibreOffice writes.
    let mut uc_count: usize = 1;
    let mut in_row = false;
    // Right edges of the current row's cells, declared by `\cellxN` after
    // `\trowd` (before or after the cells' text, depending on the writer).
    let mut cell_edges: Vec<i64> = Vec::new();
    let mut list_marker_at: Option<usize> = None;
    let mut para_start: Option<usize> = Some(0);
    let heading_style = heading_styles(&src);
    let mut pending_link: Option<String> = None;
    let mut pending_copy = false;
    // (depth of the result group, start of the text, address, inside a note).
    let mut link_open: Option<(i32, usize, String, bool)> = None;
    let mut skip_depth: i32 = 0;
    let mut depth: i32 = 0;
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                depth += 1;
                if skip_depth > 0 {
                    continue;
                }
                // Look ahead for a destination group we must drop entirely.
                let mut probe = String::new();
                let mut it = chars.clone();
                // 24 and not 12: `\*\footnote` is 11 characters, and it is
                // the `\ftnalt` that FOLLOWS which distinguishes an endnote from a
                // footnote. A window that stops at the name of the
                // destination cannot read what qualifies it.
                for _ in 0..24 {
                    match it.next() {
                        Some(ch) => probe.push(ch),
                        None => break,
                    }
                }
                // **`\\*` says « ignorable destination », and one of them is
                // CONTENT.** LibreOffice writes the footnote
                // `{\\*\\footnote …}` and the endnote `{\\*\\footnote \\ftnalt …}`,
                // so the generic rule took both of them away. MEASURED on
                // 2026-09-11 on a source converted into the three formats:
                // `notes.odt` and `notes.docx` return their five needles,
                // `notes.rtf` returned TWO — the footnote and the
                // endnote were missing, exactly as they were missing from `.docx` before
                // its notes fix. Third format, same defect: a part the body
                // does not name.
                const CONTENT_DESPITE_THE_STAR: [&str; 1] = ["\\*\\footnote"];
                // `\\pict` is NOT in this list: `rtf_pictures_as_marks` has
                // already replaced each `{\\pict …}` group with its mark before
                // this reader starts, so the entry would no longer decide anything.
                // `\\header…`/`\\footer…` (and their l/r/f page variants): page
                // furniture, left out of `.docx` and `.odt` for that reason
                // (`notes_pages`); served here alone, the same document read
                // `CONFIDENTIAL header` / `Page footer text` above its title.
                const DESTINATIONS: [&str; 9] = [
                    "\\header",
                    "\\footer",
                    "\\*",
                    "\\fonttbl",
                    "\\colortbl",
                    "\\stylesheet",
                    "\\info",
                    "\\object",
                    "\\themedata",
                ];
                // Only ever ARM the skip at a real nesting level; `depth == 0`
                // here would mean "not skipping" and leak the destination.
                let keep_anyway = CONTENT_DESPITE_THE_STAR
                    .iter()
                    .any(|d| probe.starts_with(d));
                // Word writes the SAME note without the star — `{\footnote …}`, as
                // the specification does: without it, the note was read as ordinary
                // text, glued in the middle of the sentence that calls it
                // (`crème Note body here. end.`) and without a call.
                let is_note = keep_anyway
                    || probe
                        .strip_prefix("\\footnote")
                        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphabetic()));
                // **A hyperlink is a FIELD**: `{\field{\*\fldinst HYPERLINK
                // "url"}{\fldrslt text}}`. The instruction sits in a `\*`
                // group skipped below, so the URL was lost and only `text`
                // came out — where the `.docx` and `.odt` of the same source
                // read `[text](url)`. The URL is read before the skip, and the
                // result group is wrapped when it closes.
                // In a note too: `See the source.` lost the address there
                // that the `.odt` keeps (2026-10-04).
                if depth > 0 && probe.starts_with("\\*\\fldinst") {
                    pending_link = hyperlink_target(chars.clone());
                    pending_copy =
                        super::fields::is_a_copy_of_the_text(&field_instruction(chars.clone()));
                }
                // The result of a table of contents copies the headings: skipped.
                if depth > 0 && probe.starts_with("\\fldrslt") && std::mem::take(&mut pending_copy)
                {
                    skip_depth = depth;
                    continue;
                }
                if depth > 0 && probe.starts_with("\\fldrslt") {
                    if let Some(url) = pending_link.take() {
                        let in_note = note_depth > 0;
                        let at = if in_note {
                            notes[note_kind].len()
                        } else {
                            out.len()
                        };
                        link_open = Some((depth, at, url, in_note));
                    }
                }
                if depth > 0 && !keep_anyway && DESTINATIONS.iter().any(|d| probe.starts_with(d)) {
                    skip_depth = depth;
                }
                // **A list item's marker is its `{\listtext …}` group**, and
                // LibreOffice writes the bullet as a Symbol-font private-use
                // glyph (`\u-3947`): read literally, an invisible character
                // and a tab. The group becomes a markdown marker instead — its
                // label when it carries one (`1.`, `a)`), `- ` otherwise — and
                // the item's `\ilvlN`, which comes AFTER the group, indents it.
                // `{\pntext …}` is the same group in the Word 6/95 spelling, still
                // written by WordPad and other generators: read literally it
                // served `·<tab>item` where `.docx`/`.odt` give `- item`.
                if depth > 0
                    && note_depth == 0
                    && (probe.starts_with("\\listtext") || probe.starts_with("\\pntext"))
                {
                    skip_depth = depth;
                    let label = listtext_label(chars.clone());
                    // An EMPTY label is no list: LibreOffice gives every heading
                    // an outline-numbering `{\listtext \tab}` with no number,
                    // and a `- ` there turned each heading into a bullet.
                    if !label.is_empty() {
                        let marker = if label.chars().any(char::is_alphanumeric) {
                            format!("{label} ")
                        } else {
                            "- ".to_string()
                        };
                        list_marker_at = Some(out.len());
                        out.push_str(&marker);
                    }
                }
                // A note nested in a note does not exist; the first
                // group met is the one that decides, and the next one does not
                // re-arm the class.
                if depth > 0 && is_note && note_depth == 0 {
                    note_depth = depth;
                    note_kind = usize::from(probe.contains("\\ftnalt"));
                    // The same call the Word and OpenDocument readers write
                    // (`ooxml_note_calls`): numbered per class in order.
                    note_count[note_kind] += 1;
                    let label = format!("[^{}{}]", ["", "e"][note_kind], note_count[note_kind]);
                    let _ = write!(out, "{label}");
                    let _ = write!(notes[note_kind], "{label}: ");
                }
            }
            '}' => {
                if skip_depth > 0 && depth == skip_depth {
                    skip_depth = 0;
                }
                if link_open.as_ref().is_some_and(|(d, ..)| *d == depth) {
                    if let Some((_, at, url, in_note)) = link_open.take() {
                        let buf = if in_note {
                            &mut notes[note_kind]
                        } else {
                            &mut out
                        };
                        let text = buf.get(at..).unwrap_or("").trim().to_string();
                        if !text.is_empty() {
                            buf.truncate(at);
                            let _ = write!(buf, "[{text}]({url})");
                        }
                    }
                }
                if note_depth > 0 && depth == note_depth {
                    note_depth = 0;
                    // Each note is a separate entry: without this bound, the
                    // second would glue onto the first.
                    notes[note_kind].push('\n');
                }
                // Floor at 0. An unbalanced `}` (truncated or damaged RTF) used
                // to push `depth` negative; a later destination then set
                // `skip_depth` to that negative value, which can never equal
                // the reset condition — the parser went permanently mute and
                // returned "" for the whole document. The opposite case is just
                // as bad: `depth` back at exactly 0 meant `skip_depth = 0` =
                // "not skipping", so `\info` (author, company, comments)
                // LEAKED into the agent's context.
                if raised.is_some_and(|(d, _, _)| d == depth) {
                    raised = None;
                }
                if unseen == Some(depth) {
                    unseen = None;
                }
                depth = (depth - 1).max(0);
            }
            '\\' => {
                // Escaped literal, hex byte, or control word.
                match chars.peek().copied() {
                    Some('\\') | Some('{') | Some('}') => {
                        let lit = chars.next().unwrap();
                        if skip_depth == 0 {
                            emit!(lit);
                        }
                    }
                    Some('\'') => {
                        chars.next();
                        let hex: String = (0..2).filter_map(|_| chars.next()).collect();
                        if skip_depth == 0 {
                            if let Ok(b) = u8::from_str_radix(&hex, 16) {
                                emit!(b as char); // Latin-1 range
                            }
                        }
                    }
                    // Control SYMBOLS, whose name is a single punctuation
                    // character. The word scanner below only takes
                    // alphanumerics, so it collected nothing and left the
                    // punctuation to be emitted as text: `\~`, a non-breaking
                    // space, came out as a literal `~` inside every quotation
                    // the document had.
                    // `\\*` marks « the following destination is ignorable ». It is
                    // not text, and since `{\\*\\footnote …}` is no longer skipped
                    // the asterisk landed IN the sentence: « carries the
                    // argument.*\tNEEDLE-NOTE-FOOT ». A character the document does not
                    // contain, fabricated by the neighbouring fix — same
                    // class as the `~` the next arm exists to extinguish.
                    Some('*') => {
                        chars.next();
                    }
                    Some('~') => {
                        chars.next();
                        if skip_depth == 0 {
                            emit!(' ');
                        }
                    }
                    Some('_') => {
                        chars.next();
                        if skip_depth == 0 {
                            emit!('-');
                        }
                    }
                    _ => {
                        let mut word = String::new();
                        while let Some(&ch) = chars.peek() {
                            if ch.is_ascii_alphanumeric() || ch == '-' {
                                word.push(ch);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        // One space after a control word is a delimiter.
                        if chars.peek() == Some(&' ') {
                            chars.next();
                        }
                        // `\uNNNN` — the ONLY way Word/LibreOffice encode a
                        // character outside the codepage, i.e. every Cyrillic,
                        // CJK, Greek and Hebrew document. Unhandled, the digits
                        // were stripped as a numeric suffix and the ASCII
                        // fallback that follows was emitted instead: "Привет"
                        // came out as "??????" — a successful-looking
                        // extraction of nothing.
                        let is_unicode = word.starts_with('u')
                            && word.len() > 1
                            && word[1..].chars().all(|c| c.is_ascii_digit() || c == '-');
                        if is_unicode {
                            if let Ok(n) = word[1..].parse::<i32>() {
                                // Negative values are the signed-16-bit form.
                                let cp = if n < 0 { (n + 65536) as u32 } else { n as u32 };
                                if skip_depth == 0 {
                                    if let Some(ch) = char::from_u32(cp) {
                                        emit!(ch);
                                    }
                                }
                            }
                            // Skip the fallback that follows, as many units as
                            // the document's `\ucN` declares (1 by default, 0
                            // meaning there is none to skip).
                            for _ in 0..uc_count {
                                if !skip_rtf_fallback_unit(&mut chars) {
                                    break;
                                }
                            }
                        } else if word.starts_with("uc")
                            && word.len() > 2
                            && word[2..].chars().all(|c| c.is_ascii_digit())
                        {
                            // `\ucN` declares how many fallback units follow
                            // each `\uN`. Word writes `\uc0` when it emits no
                            // fallback at all, and skipping one there would eat
                            // a REAL character.
                            uc_count = word[2..].parse::<usize>().unwrap_or(1);
                        } else if skip_depth == 0 {
                            let w = word.trim_end_matches(|c: char| c.is_ascii_digit());
                            match w {
                                // Inside a table row a paragraph break belongs
                                // to the CELL: a newline there would cut the row.
                                "par" | "line" if in_row => emit!(' '),
                                "par" | "line" | "sect" | "page" => {
                                    list_marker_at = None;
                                    emit!('\n');
                                    para_start = Some(out.len());
                                }
                                // A heading declares its level in its paragraph
                                // properties, before its text: `# ` is put at the
                                // paragraph's start, like `\ilvl` indents a
                                // marker after the fact. Ignored, the three
                                // levels of a LibreOffice export read as prose.
                                "outlinelevel" if note_depth == 0 && !in_row => {
                                    if let Some(at) = para_start.take() {
                                        let level: usize = word[12..].parse().unwrap_or(0).min(5);
                                        out.insert_str(at, &format!("{} ", "#".repeat(level + 1)));
                                    }
                                }
                                // The paragraph's STYLE can be a heading too:
                                // see `heading_styles`.
                                "s" if note_depth == 0 && !in_row && word.len() > 1 => {
                                    let level = word[1..]
                                        .parse::<u32>()
                                        .ok()
                                        .and_then(|n| heading_style.get(&n));
                                    if let (Some(&level), Some(at)) = (level, para_start) {
                                        para_start = None;
                                        out.insert_str(
                                            at,
                                            &format!("{} ", "#".repeat(level.min(6))),
                                        );
                                    }
                                }
                                "ilvl" => {
                                    let level: usize = word[4..].parse().unwrap_or(0).min(8);
                                    if let Some(at) = list_marker_at.take() {
                                        out.insert_str(at, &"  ".repeat(level));
                                    }
                                }
                                "tab" => emit!('\t'),
                                "v" | "deleted" => {
                                    if word[w.len()..] == *"0" {
                                        unseen = None;
                                    } else if unseen.is_none() {
                                        unseen = Some(depth);
                                    }
                                }
                                "plain" => unseen = None,
                                "super" => raised = Some((depth, '^', false)),
                                "sub" => raised = Some((depth, '_', false)),
                                "nosupersub" => raised = None,
                                // A table: `\intbl` opens row content, `\cell`
                                // ends a cell, `\row` ends the row. Ignored, the
                                // cells were glued into words the document
                                // never wrote (`ItemPriceWidget…`, 2026-10-03).
                                "intbl" => in_row = true,
                                "cell" => emit!(CELL_END),
                                "trowd" => cell_edges.clear(),
                                "cellx" => cell_edges.push(word[5..].parse().unwrap_or(0)),
                                "row" => {
                                    in_row = false;
                                    emit!(ROW_EDGES);
                                    let edges = cell_edges
                                        .iter()
                                        .map(i64::to_string)
                                        .collect::<Vec<_>>()
                                        .join(",");
                                    for ch in edges.chars() {
                                        emit!(ch);
                                    }
                                    emit!('\n');
                                }
                                // A table INSIDE a cell (`\nestcell`, `\nestrow`)
                                // reads inside that cell: taken as `\cell`/`\row`
                                // its cells widened the outer row and its row
                                // closed it (`| Outer A2 | In1 | In2 | In3 | In4 |`,
                                // 2026-10-04) — the `.doc` and the `.docx` of the
                                // same file keep `In1 In2 In3 In4` in the cell.
                                "nestcell" | "nestrow" => emit!(' '),
                                // `\binN` is followed by N RAW bytes of an
                                // embedded object; without consuming them their
                                // printable content lands in the text.
                                "bin" => {
                                    let n: usize = word[3..].parse().unwrap_or(0);
                                    for _ in 0..n {
                                        if chars.next().is_none() {
                                            break;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            '\n' | '\r' => {} // raw newlines are formatting, not content
            _ => {
                if skip_depth == 0 {
                    if let Some((_, mark, decided @ false)) = raised.as_mut() {
                        if !c.is_whitespace() {
                            *decided = true;
                            if !c.is_alphabetic() {
                                emit!(*mark);
                            }
                        }
                    }
                    emit!(c);
                }
            }
        }
    }
    // The notes at the end of the text, under the same headings as the other two
    // format families — the same source converted must read the same.
    for (label, text) in super::NOTE_SECTIONS.iter().zip(&notes) {
        if text.trim().is_empty() {
            continue;
        }
        let _ = write!(out, "\n\n## {label}\n{}\n", text.trim());
    }
    let out = rows_as_tables(&out);
    // Collapse the blank-line runs the control words leave behind.
    let mut cleaned = String::with_capacity(out.len());
    let mut blanks = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        cleaned.push_str(line.trim_end());
        cleaned.push('\n');
    }
    cleaned.trim().to_string()
}

#[cfg(test)]
mod a_table_is_a_table {
    use super::extract_rtf;

    /// `\cell` and `\row` were ignored: the cells came back glued into words
    /// the document never wrote (`ItemPriceWidget…`).
    #[test]
    fn a_heading_style_marks_its_paragraph() {
        let rtf = r"{\rtf1{\stylesheet{\s0 Normal;}{\s1\sbasedon0\b heading 1;}{\s2\b\fs26 Heading 2;}{\s5 Titre 3;}}
\pard\plain\s1\sb480 Report\par
\pard\plain\s2 Results\par
\pard\plain\s5 Latence\par
\pard\plain\s0 Body text.\par}";
        let out = extract_rtf(rtf.as_bytes());
        assert_eq!(
            out.trim(),
            "# Report\n## Results\n### Latence\nBody text.",
            "{out:?}"
        );
    }

    /// A table inside a cell (`\itap2`, `\nestcell`, `\nestrow`, the props in
    /// `{\*\nesttableprops}`) reads inside that cell, not as more columns.
    #[test]
    fn a_nested_table_reads_inside_its_cell() {
        let rtf = br"{\rtf1\ansi\trowd\intbl Outer A1\cell\intbl Outer B1\cell\row
\trowd\intbl Outer A2\cell
\pard\intbl\itap2 In1\nestcell In2\nestcell{\*\nesttableprops\trowd\nestrow}
\pard\intbl\itap2 In3\nestcell In4\nestcell{\*\nesttableprops\trowd\nestrow}
\pard\intbl\itap1\cell\row
After.\par}";
        let out = extract_rtf(rtf);
        assert!(out.contains("| Outer A2 | In1 In2 In3 In4 |"), "{out}");
        assert!(
            !out.contains("| In3 |") && !out.contains("| In2 |"),
            "{out}"
        );
    }

    /// A cell merged across two columns mid-row (`\cellx` edges 2052, 6372
    /// against the header's 2052, 4212, 6372) keeps the cells after it under
    /// their own headers.
    #[test]
    fn a_cell_merged_mid_row_keeps_the_next_cells_in_their_columns() {
        let rtf = br"{\rtf1\ansi
\trowd\cellx2052\cellx4212\cellx6372\cellx8532\intbl Item\cell\intbl Q1\cell\intbl Q2\cell\intbl Total\cell\row
\trowd\cellx2052\cellx6372\cellx8532\intbl Gadget\cell\intbl pending\cell\intbl 99\cell\row
After.\par}";
        let out = extract_rtf(rtf);
        assert!(out.contains("| Gadget | pending |  | 99 |"), "{out}");
        assert!(!out.contains('\u{1E}') && out.contains("After."), "{out:?}");
    }

    #[test]
    fn cells_and_rows_become_a_markdown_table() {
        let rtf = br"{\rtf1\ansi Before.\par
\trowd\intbl Item\cell\intbl Price\cell\row
\trowd\intbl Widget\par second line\cell\intbl 12.50\cell\row
After.\par}";
        let out = extract_rtf(rtf);
        assert!(out.contains("| Item | Price |"), "{out}");
        assert!(out.contains("| Widget second line | 12.50 |"), "{out}");
        assert!(out.contains("After."), "{out}");
        assert!(!out.contains("ItemPrice"), "{out}");
    }
}

#[cfg(test)]
mod a_list_keeps_its_markers_and_levels {
    use super::extract_rtf;

    /// The bullet was a private-use glyph and a tab, and `\ilvlN` (written
    /// AFTER the marker group) was ignored: a nested item read as a sibling.
    #[test]
    fn bullets_numbers_and_nesting() {
        let rtf = br"{\rtf1\ansi
{\listtext\pard\plain \u-3947\'3f\tab}\ilvl0\ls1 Phase one\par
{\listtext\pard\plain \u-3947\'3f\tab}\ilvl1\ls1 Design review\par
{\listtext\pard\plain 2.\tab}\ilvl0\ls2 Second step\par
{\listtext\pard\plain \tab}\outlinelevel0 Heading text\par
Plain paragraph.\par}";
        let out = extract_rtf(rtf);
        assert!(
            out.contains("- Phase one\n  - Design review\n2. Second step"),
            "{out:?}"
        );
        assert!(
            !out.contains("- Heading text"),
            "an empty listtext is no bullet: {out:?}"
        );
        assert!(
            out.contains("\n# Heading text\n"),
            "outline level 0 is a heading: {out:?}"
        );
        assert!(out.contains("\nPlain paragraph."), "{out:?}");
    }
    /// The Word 6/95 spelling, `{\pntext …}`, still written by WordPad: the
    /// same marker group, the same rendering as `{\listtext …}`.
    #[test]
    fn the_legacy_pntext_group_is_a_marker_too() {
        let rtf = br"{\rtf1\ansi{\fonttbl{\f0 Arial;}{\f1 Symbol;}}
\pard{\pntext\f1 \'b7\tab}{\*\pn\pnlvlblt\pnf1\pnindent0{\pntxtb\'b7}}\fi-360\li720 First bullet\par
{\pntext\f1 \'b7\tab}Second bullet\par
\pard{\pntext\f0 1.\tab}{\*\pn\pnlvlbody\pndec}First step\par
\pard Plain.\par}";
        let out = extract_rtf(rtf);
        assert!(
            out.contains("- First bullet\n- Second bullet\n1. First step"),
            "{out:?}"
        );
        assert!(!out.contains('\u{b7}'), "the glyph leaked: {out:?}");
    }

    /// Page furniture is not the document, as for `.docx` and `.odt`; a
    /// footnote (`\footnote`, which shares the `\foot` prefix) still is.
    #[test]
    fn headers_and_footers_are_furniture() {
        let rtf = br"{\rtf1\ansi{\header\pard CONFIDENTIAL header\par}{\footerr\pard Page footer text\par}
\pard Body text{\footnote\pard Note kept.}\par}";
        let out = extract_rtf(rtf);
        assert!(
            !out.contains("CONFIDENTIAL") && !out.contains("Page footer"),
            "{out:?}"
        );
        assert!(
            out.contains("Body text") && out.contains("Note kept."),
            "{out:?}"
        );
    }

    /// Hidden (`\v`) and track-deleted (`\deleted`) text is not what Word
    /// shows: out until its group closes, `\v0`, or `\plain` — and the
    /// paragraph it sat in is not welded to the next.
    #[test]
    fn hidden_and_deleted_text_stay_out() {
        let rtf = br"{\rtf1\ansi\pard Visible start {\v SECRET} visible end.\par
\pard Kept {\deleted GONE}tail\par
\pard Plain \v hid\plain shown, \v again\v0 back\par
\pard Before{\v gone\par}After\par}";
        let lines: Vec<String> = extract_rtf(rtf)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(
            lines,
            [
                "Visible start  visible end.",
                "Kept tail",
                "Plain shown, back",
                "Before",
                "After"
            ]
        );
    }
}

#[cfg(test)]
mod a_hyperlink_keeps_its_target {
    use super::extract_rtf;

    /// The URL lives in a `\*\fldinst` group the reader skips: only the text
    /// came out, where `.docx`/`.odt` read `[text](url)`.
    #[test]
    fn an_external_link_is_a_markdown_link_and_an_anchor_is_not() {
        let rtf = br#"{\rtf1\ansi Read the {\field{\*\fldinst HYPERLINK "https://example.org/docs" }{\fldrslt {\ul full documentation}}} first. See {\field{\*\fldinst HYPERLINK \l "sec2" }{\fldrslt section two}} below.\par}"#;
        let out = extract_rtf(rtf);
        assert!(
            out.contains("Read the [full documentation](https://example.org/docs) first."),
            "{out:?}"
        );
        assert!(out.contains("See section two below."), "{out:?}");
    }

    /// Word writes a note WITHOUT the `\*` LibreOffice puts before it.
    #[test]
    fn a_word_note_without_the_star_is_a_note_not_a_sentence() {
        let rtf = br#"{\rtf1\ansi Caf\'e9 cr\'e8me{\super\chftn{\footnote\pard\plain{\super\chftn} Note body here.}} end. Last{\super\chftn{\footnote\ftnalt\pard\plain{\super\chftn} End note.}}.\par}"#;
        let out = extract_rtf(rtf);
        assert!(out.contains("Café crème[^1] end. Last[^e1]."), "{out:?}");
        assert!(
            out.contains("## Footnotes\n[^1]: Note body here."),
            "{out:?}"
        );
        assert!(out.contains("## Endnotes\n[^e1]: End note."), "{out:?}");
    }

    /// A jump to a bookmark (`HYPERLINK "#intro"`) has no address and reads
    /// as plain text, as in `.docx` and `.odt`.
    #[test]
    fn a_link_to_a_bookmark_is_plain_text() {
        let rtf = r##"{\rtf1 Back to {\field{\*\fldinst HYPERLINK "#intro"}{\fldrslt the intro}} or {\field{\*\fldinst HYPERLINK "https://e.x/"}{\fldrslt the site}}.}"##;
        assert_eq!(
            extract_rtf(rtf.as_bytes()).trim(),
            "Back to the intro or [the site](https://e.x/)."
        );
    }

    /// A link inside a footnote keeps its address, as in the body.
    #[test]
    fn a_link_inside_a_footnote_keeps_its_address() {
        let rtf = r##"{\rtf1 Note here{\super\chftn}{\*\footnote \chftn See {\field{\*\fldinst HYPERLINK "https://note.example.com/"}{\fldrslt the source}}.} end.}"##;
        let out = extract_rtf(rtf.as_bytes());
        assert!(
            out.contains("[^1]: See [the source](https://note.example.com/)."),
            "{out:?}"
        );
    }

    /// A table of contents copies the headings with their page numbers: left
    /// out, as `.docx`, `.odt` and `.doc` leave it out.
    #[test]
    fn a_table_of_contents_is_not_served() {
        let rtf = r#"{\rtf1 {\field{\*\fldinst { TOC \\o "1-3" \\h}}{\fldrslt {{STALE Chapter One ..... 1}\par }}}Chapter One\par Body.}"#;
        assert_eq!(extract_rtf(rtf.as_bytes()).trim(), "Chapter One\nBody.");
    }
}
