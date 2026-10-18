//! `free` / `free -h` / `free -m` — memory usage.
//!
//! ```text
//!                total        used        free      shared  buff/cache   available
//! Mem:           31984        21456        1234        1024        9293        9123
//! Swap:          16384         384        16000
//! ```
//!
//! Output is tiny (3 lines), so there is no tail to fold. The only real
//! waste is the column-alignment padding (`free` right-justifies every
//! field into a wide fixed grid). We collapse runs of spaces to one,
//! which an agent never needs, and keep every line verbatim otherwise.
//!
//! Detection is structural and locale-independent: the memory row label
//! `Mem:` is NOT translated by util-linux (only the header words and the
//! swap label are — under `LANG=fr_FR` the header reads
//! `total utilisé libre partagé tamp/cache disponible` and swap becomes
//! `Échange:`, but `Mem:` stays put). Matching English header words
//! (`used`/`free`) as the old detector did silently failed everywhere
//! but English.


/// True when a line's first whitespace-delimited token is the (untranslated)
/// `free` memory-row label, e.g. `Mem:`. Some builds also emit `Low:`/`High:`.
fn is_mem_row(line: &str) -> bool {
    matches!(
        line.split_whitespace().next(),
        Some("Mem:") | Some("Low:") | Some("High:") | Some("Total:")
    )
}

pub fn detect(text: &str) -> bool {
    text.lines().any(is_mem_row)
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    // The rule has nothing specific to `free`: it holds for any aligned
    // output. It lives in `archetypes`, and the language-free fallback of
    // `auto` uses it for the tools whose compactor declined.
    crate::archetypes::collapse_alignment_padding(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = "              total        used        free      shared\nMem:           31984       21456        1234        1024\nSwap:          16384         384       16000\n";
    // Real `free -h` under LANG=fr_FR.UTF-8 — header + swap label localised,
    // `Mem:` is not.
    const FR: &str = "               total       utilisé      libre     partagé tamp/cache   disponible\nMem:            61Gi        15Gi       5,7Gi       4,7Gi        46Gi        46Gi\nÉchange:          0B          0B          0B\n";

    #[test]
    fn detects_english() {
        assert!(detect(EN));
    }

    #[test]
    fn detects_french_via_mem_label() {
        assert!(detect(FR));
    }

    #[test]
    fn rejects_unrelated() {
        assert!(!detect("just some prose\nwith no memory row\n"));
    }

    #[test]
    fn collapses_alignment_padding() {
        let out = compact(FR);
        assert!(out.contains("Mem: 61Gi 15Gi"), "padding not collapsed:\n{out}");
        // Localised swap label preserved verbatim.
        assert!(out.contains("Échange: 0B 0B 0B"));
        // Field contents (including the FR decimal comma) untouched.
        assert!(out.contains("5,7Gi"));
        assert!(out.len() < FR.len());
    }
}
