//! **A thousand lines that differ only by their values.**
//!
//! A log is made of TEMPLATES: the same sentence with a timestamp, an
//! identifier, an address that change. Recognising the template lets us
//! say the sentence once and the values separately.
//!
//! The slots are typed — timestamp, MAC, UUID, path — because an
//! over-generous slot swallows a word that carried meaning, and a
//! swallowed value shows up in no report.

// No `use super::*;`: recognising a template is done on bare text,
// without knowing anything about curl or grouping.

/// One variable-token category recognised by the templater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SlotKind {
    Ts,
    Num,
    Uuid,
    Hex,
    /// A MAC address: six 2-digit hex groups. A DECLARED format, like the
    /// UUID beside it — not a guess about entropy.
    Mac,
    /// File-path-like token (gated). Lets lines that differ only by path —
    /// e.g. per-file lint/compiler output — fold to one template.
    Path,
}

impl SlotKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SlotKind::Ts => "TS",
            SlotKind::Num => "N",
            SlotKind::Uuid => "UUID",
            SlotKind::Hex => "HEX",
            SlotKind::Mac => "MAC",
            SlotKind::Path => "PATH",
        }
    }
    pub(crate) fn placeholder(self) -> &'static str {
        match self {
            SlotKind::Ts => "<TS>",
            SlotKind::Num => "<N>",
            SlotKind::Uuid => "<UUID>",
            SlotKind::Hex => "<HEX>",
            SlotKind::Mac => "<MAC>",
            SlotKind::Path => "<PATH>",
        }
    }
}

/// Whether the gated path slot is enabled (`TOKENADE_LOG_PATH_SLOT=1`). Cached
/// — `walk_line` is hot, so we read the env once.
pub(crate) fn path_slot_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("TOKENADE_LOG_PATH_SLOT").is_ok_and(|v| v != "0" && !v.is_empty())
    })
}

/// If a file-path-like token starts at `bytes[i]`, return its end index. A path
/// token is a run of `[A-Za-z0-9._/-]` that contains a `/` and a `.ext` after
/// the last `/` (so `src/foo.rs` matches but `12-34` / `a/b` do not). Length
/// 5..=200 to avoid trivial or runaway matches.
pub(crate) fn path_token_end(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    while j < bytes.len() {
        let c = bytes[j];
        if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'-') {
            j += 1;
        } else {
            break;
        }
    }
    let tok = &bytes[i..j];
    if tok.len() < 5 || tok.len() > 200 {
        return None;
    }
    let slash = tok.iter().rposition(|&c| c == b'/')?;
    // must have a `.ext` (dot followed by ≥1 alnum) after the last slash.
    let after = &tok[slash + 1..];
    let dot = after.iter().position(|&c| c == b'.')?;
    if after[dot + 1..]
        .iter()
        .take(1)
        .all(|c| c.is_ascii_alphanumeric())
        && dot + 1 < after.len()
    {
        Some(j)
    } else {
        None
    }
}

/// Walk `line`, recognising the same variable tokens as `template_of`
/// (TS / N / UUID / HEX). For each match, invoke `on_slot(kind,
/// original_value, slot_index)` and emit its return string. Non-slot
/// bytes are emitted verbatim. Centralises the byte-walking logic so
/// `template_of`, `slots_of`, and the per-slot inliner all stay in
/// lockstep — if one teaches the walker about a new token shape the
/// others get it for free.
pub(crate) fn walk_line<F>(line: &str, mut on_slot: F) -> String
where
    F: FnMut(SlotKind, &str, usize) -> String,
{
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut slot_idx = 0usize;
    while i < bytes.len() {
        // ISO-8601 timestamp prefix.
        if i + 19 <= bytes.len() && is_iso_timestamp_at(bytes, i) {
            let start = i;
            i += 19;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || matches!(bytes[i], b'.' | b'Z' | b'+' | b'-' | b':'))
            {
                if bytes[i] == b' ' {
                    break;
                }
                i += 1;
            }
            let val = &line[start..i];
            out.push_str(&on_slot(SlotKind::Ts, val, slot_idx));
            slot_idx += 1;
            continue;
        }
        // Path slot (gated): word-boundary anchored so we only fold a whole
        // file path, never a fragment of another token.
        if path_slot_enabled() && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            if let Some(end) = path_token_end(bytes, i) {
                let val = &line[i..end];
                out.push_str(&on_slot(SlotKind::Path, val, slot_idx));
                slot_idx += 1;
                i = end;
                continue;
            }
        }
        let b = bytes[i];
        // BEFORE the number pass, both of them. That pass claims the leading
        // digits of a token, which is how `00:1a:…` became `<N>:<N>a:…` and
        // how `550e8400-e29b-…` became `<N>e<N>-e29b-…`: only a value opening
        // with a hex LETTER ever reached a check placed after it, and about
        // five in eight uuids open with a digit. MEASURED on 300 log lines
        // differing only by a real uuid: 9.3% saved with the check after the
        // number pass, 98% with it before.
        if b.is_ascii_hexdigit() && i + 36 <= bytes.len() && is_uuid_at(bytes, i) {
            let val = &line[i..i + 36];
            out.push_str(&on_slot(SlotKind::Uuid, val, slot_idx));
            slot_idx += 1;
            i += 36;
            continue;
        }
        // BEFORE the number pass: that pass claims the leading digits of each
        // group, which is exactly how `00:1a:…` ended up as `<N>:<N>a:…`.
        // Only an all-letter address ever reached a check placed after it.
        if b.is_ascii_hexdigit() && is_mac_at(bytes, i) {
            let val = &line[i..i + 17];
            out.push_str(&on_slot(SlotKind::Mac, val, slot_idx));
            slot_idx += 1;
            i += 17;
            continue;
        }
        // A HASH that opens with a digit, for the same reason as the uuid and
        // mac checks above: the number pass took `57` of `57faf2d2def6`, so a
        // docker netns, a volume id, a short commit were `<N><HEX>` — two slots
        // for one id, `distinct N=616` counting nothing (2026-10-03, a real
        // `docker volume ls` of 2 288 volumes). A whole token: bounded both
        // sides, eight hex digits or more, at least one of them a letter.
        if b.is_ascii_digit() && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            let end = i + bytes[i..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            let bounded = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if end - i >= 8 && bounded && bytes[i..end].iter().any(u8::is_ascii_alphabetic) {
                out.push_str(&on_slot(SlotKind::Hex, &line[i..end], slot_idx));
                slot_idx += 1;
                i = end;
                continue;
            }
        }
        if b.is_ascii_digit() {
            let start = i;
            while i < bytes.len() {
                if bytes[i].is_ascii_digit() {
                    i += 1;
                    continue;
                }
                // A `.` or `-` belongs to the NUMBER only when a digit follows
                // it: `1.5`, `1.0.7-1`, `192.168.0.1`, `2026-08-25`. Absorbing
                // one unconditionally is how `f300.txt` came out as
                // `f<N>txt` — the dot vanished into the slot, and an agent
                // rebuilding the name from the fold gets `f300txt` and
                // concludes the file it asked about is not there. That is the
                // false-absence class arriving through the templater instead
                // of through truncation. The comment below records the same
                // greed biting once before, on `f412.cpp.o`.
                if matches!(bytes[i], b'.' | b'-')
                    && i + 1 < bytes.len()
                    && bytes[i + 1].is_ascii_digit()
                {
                    i += 1;
                    continue;
                }
                break;
            }
            // A short digit run glued to letters is kept verbatim ONLY when
            // the digit is EMBEDDED in a letter-led identifier (utf8, k8s,
            // i18n, sha256) — i.e. preceded by a letter. A digit that STARTS
            // the token and is followed by letters is a number+unit
            // (`0ms`, `12ms`, `500KB`, `50%`): slot the number, keep the unit,
            // so `latency=0ms` / `latency=1ms` collapse to one template.
            let embedded_in_identifier = start > 0 && bytes[start - 1].is_ascii_alphabetic();
            // …and the run must be DIGITS ONLY. The loop above also swallows
            // `.` and `-`, so `f412.cpp.o` produced the run `412.` — preceded
            // by `f`, followed by `c` — and was kept verbatim as if it were
            // `utf8`. Every such path then templated to itself, so a 600-line
            // build log folded to nothing while `libpkg7 (1.0.7-1)` folded
            // fine. A separator means the digits are their own token, whatever
            // sits either side of them.
            let pure_digits = line[start..i].bytes().all(|c| c.is_ascii_digit());
            if i - start <= 3
                && i < bytes.len()
                && bytes[i].is_ascii_alphabetic()
                && embedded_in_identifier
                && pure_digits
            {
                out.push_str(&line[start..i]);
                continue;
            }
            // Padding that right-aligns a number is PRESENTATION, not part of
            // the sentence. `[   9%]` and `[  10%]` are the same line said
            // twice, and keeping the spaces literal put them in different
            // templates: MEASURED on a 632-line cmake build, every one of the
            // 10 modules was listed TWICE — once for its 1-digit percentages
            // and once for its 2-and-3-digit ones — 20 rows where 10 say the
            // same thing, under a banner announcing 32 shapes for 10.
            //
            // The padding is the spaces BEYOND THE FIRST, and exactly one is
            // always kept. The separator between a word and a number is the
            // sentence, not decoration: absorbing the whole run turned
            // `conn from 10.0.0.5` into `conn from<N>`, which two existing
            // tests caught and were right to.
            //
            // Only INTERIOR padding is absorbed. Leading indentation is
            // structure (a nested block, a stack frame, a tree), so spaces
            // with nothing but spaces before them on the line are kept: the
            // test holds both halves apart.
            if out.bytes().any(|c| c != b' ') {
                while out.ends_with("  ") {
                    out.pop();
                }
            }
            let val = &line[start..i];
            out.push_str(&on_slot(SlotKind::Num, val, slot_idx));
            slot_idx += 1;
            continue;
        }
        if b.is_ascii_hexdigit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - start >= 8 {
                let val = &line[start..i];
                out.push_str(&on_slot(SlotKind::Hex, val, slot_idx));
                slot_idx += 1;
                continue;
            }
            // Too short to be a hash — so do NOT swallow it. `a`–`f` are hex
            // digits, which meant this branch ate `f0`, `f412` and `formula0`
            // whole and emitted them verbatim, and the numbers inside them were
            // never slotted. Every line of a build log then templated to itself:
            // `libpkg7 (1.0.7-1)` folded while `f412.cpp.o` did not, for no
            // reason a reader of either line could see.
            //
            // Advance ONE character and let the walker decide again. The digit
            // branch keeps its own rule for `utf8` / `k8s` / `i18n` — digits
            // FOLLOWED by a letter stay verbatim — so identifiers are still
            // safe; what changes is that a number at the end of a token is now
            // treated as the number it is.
            out.push_str(&line[start..start + 1]);
            i = start + 1;
            continue;
        }
        let rest = &line[i..];
        if let Some(ch) = rest.chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

/// Capture every variable-token value in `line`, in encounter order.
/// Used by the fold renderer to compute per-slot distinct-value counts
/// and pick a representative sample.
pub(crate) fn slots_of(line: &str) -> Vec<(SlotKind, String)> {
    let mut slots: Vec<(SlotKind, String)> = Vec::new();
    walk_line(line, |kind, val, _idx| {
        slots.push((kind, val.to_string()));
        String::new()
    });
    slots
}

/// Template form of a line: every variable token replaced with its
/// canonical placeholder. Two lines that template-equal can be folded.
pub(crate) fn template_of(line: &str) -> String {
    walk_line(line, |kind, _val, _idx| kind.placeholder().to_string())
}

// #555 — legacy hand-rolled walker `template_of_legacy` removed;
// `template_of` above (using the unified `walk_line`) is the only
// caller. Was kept alive only via `#[allow(dead_code)]`.

/// True when `b[i..]` begins with a canonical 36-char UUID
/// (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`). Case-insensitive on the
/// hex digits. Caller must have already bounds-checked that 36 bytes
/// are available.
/// The 19 bytes at `i` are `YYYY-MM-DD` + `T` or a space + `HH:MM:SS`.
///
/// EVERY position is checked, exactly like [`is_uuid_at`] below — and beyond
/// being the correct test, that is what makes the jump that follows sound. The
/// walker does `i += 19` and then slices `&line[start..i]`, so a multibyte
/// character anywhere in the window leaves `i` inside a character and the
/// slice PANICS. The old test looked at the five separators and the FIRST
/// digit only, so the seconds field could hold anything at all. MEASURED on
/// 1.1.9: a log line `2026-09-07 12:00:0é0 INFO service=api …` killed
/// `tokenade read` with "byte index 19 is not a char boundary". Found by
/// injecting a multibyte character at every offset of ordinary log shapes.
pub(crate) fn is_iso_timestamp_at(b: &[u8], i: usize) -> bool {
    const DIGITS: [usize; 14] = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    DIGITS.iter().all(|&o| b[i + o].is_ascii_digit())
        && b[i + 4] == b'-'
        && b[i + 7] == b'-'
        && (b[i + 10] == b'T' || b[i + 10] == b' ')
        && b[i + 13] == b':'
        && b[i + 16] == b':'
}

/// True when `b[i..]` begins with a MAC address — six 2-digit hex groups
/// separated by `:` or `-`, seventeen bytes, all one separator.
///
/// A DECLARED format, exactly like [`is_uuid_at`]: the test is the shape the
/// format defines, not how random the bytes look. Without it a MAC is torn up
/// by the surrounding rules — the number pass eats each group's leading
/// digits and the hex LETTERS are left in the template, so
/// `00:1a:2b:3c:4d:5e` becomes `<N>:<N>a:<N>b:<N>c:<N>d:<N>e` and every
/// address templates differently, while an all-letter `aa:bb:cc:dd:ee:ff`
/// matches nothing at all and is kept whole. MEASURED over 300 log lines
/// differing only by the MAC: 50.3% saved, against 98% for the same lines
/// differing only by an integer, a uuid, a path or an IP.
///
/// Every position is checked, which is also what makes the `i += 17` that
/// follows sound: the whole window is ASCII by construction (see
/// `is_iso_timestamp_at` for the same reasoning, and for the panic that
/// happens when a window is stepped over without checking it).
pub(crate) fn is_mac_at(b: &[u8], i: usize) -> bool {
    const SEPS: [usize; 5] = [2, 5, 8, 11, 14];
    const HEXES: [usize; 12] = [0, 1, 3, 4, 6, 7, 9, 10, 12, 13, 15, 16];
    if i + 17 > b.len() {
        return false;
    }
    let sep = b[i + 2];
    if sep != b':' && sep != b'-' {
        return false;
    }
    if !SEPS.iter().all(|&o| b[i + o] == sep) {
        return false;
    }
    if !HEXES.iter().all(|&o| b[i + o].is_ascii_hexdigit()) {
        return false;
    }
    // A WHOLE token, not a slice of something longer, on BOTH sides. The
    // trailing test alone is not enough: in `00:1a:2b:3c:4d:5e:6f` the number
    // pass consumes the leading `00`, and the six groups left behind then
    // matched — `g <N>:<MAC> x`. Caught by this test's own seven-group arm.
    let before_ok = match i.checked_sub(1).and_then(|k| b.get(k)) {
        None => true,
        Some(c) => !c.is_ascii_hexdigit() && *c != b':' && *c != b'-',
    };
    before_ok && !matches!(b.get(i + 17), Some(c) if c.is_ascii_hexdigit() || *c == sep)
}

pub(crate) fn is_uuid_at(b: &[u8], i: usize) -> bool {
    let segments = [(0, 8), (9, 13), (14, 18), (19, 23), (24, 36)];
    for (start, end) in segments {
        for off in start..end {
            if !b[i + off].is_ascii_hexdigit() {
                return false;
            }
        }
    }
    b[i + 8] == b'-' && b[i + 13] == b'-' && b[i + 18] == b'-' && b[i + 23] == b'-'
}
